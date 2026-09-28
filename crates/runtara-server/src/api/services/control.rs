//! Native control service behind `runtara:control/api`.
//!
//! The component host runs the approved control agent in fresh stores and
//! hands every `api` call here with the caller's authority (tenant, calling
//! instance, operation) taken from the calling run, never from arguments.
//!
//! - Reads (`get`, `query`, `list-pending-signals`) cover the caller's tenant.
//! - Mutations (`send-signal`, `cancel`, `pause`, `resume`) check, in order:
//!   a calling instance (`requires-instance`), an operation scope
//!   (`requires-operation`), their arguments, then decision D1 through
//!   [`decide`]: `send-signal` reaches children, ancestors and requests that
//!   opt in with `action.key`; the lifecycle commands reach direct children
//!   only; nothing targets the caller itself. They are replay-safe through
//!   intent-first, success-only receipts keyed by `(caller, op_hash)`, and
//!   every attempt is audited without payloads.
//! - `start` durably admits a child of the caller through the execution
//!   engine ([`ExecutionEngine::start_child`]): idempotent per operation,
//!   run labels unique per parent, lineage depth 16, capacity per decision
//!   D5. The parent link, in the runtime's instance rows and in the server's
//!   admission records for children not launched yet, is what relates a
//!   target to the caller: `child` when the caller started it, `ancestor`
//!   when it is in the caller's lineage.
//! - `get` and `query(parent)` include children still in admission as
//!   `queued`; `query(parent)` merges them with the launched ones in one
//!   runtime read paged by admission time.
//! - Waiting on children is not a control call: a `WaitForInstances` step
//!   calls the [`InstanceWaits`] service, which this service carries so the
//!   runtime, the engine and the relations reach both.
//!
//! The service is late-bound: the executor exists before the embedded
//! runtime and the execution engine do, so a call waits up to
//! [`INSTALL_WAIT`] for [`NativeControl::install`] (and `start` for
//! [`NativeControl::install_engine`]) and is `unavailable` after that.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use runtara_component_host::control_host::{
    CancelRequest, CommandResult, ControlAuthority, ControlError, ControlErrorCode, ControlHost,
    InstanceDetail, InstancePage, InstanceStatus, InstanceSummary, ParentFilter, PendingSignal,
    PendingSignalPage, PendingSignalsRequest, QueryRequest, SendSignalRequest, SendSignalResult,
    SignalScope, SortField, SortOrder, StartRequest, StartResult, SuspensionReason, TerminalResult,
};
use runtara_control_contract as contract;
use runtara_environment::control_reads::ControlInstance;
use runtara_environment::instance_repository::ListInstancesOptions;
use serde_json::{Value, json};

use super::instance_waits::InstanceWaits;
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::{
    self, CommandEffect, ExecutionEngine, ExecutionError, PauseOutcome, ResumeOutcome,
    StartChildError, StopOutcome,
};
use crate::workers::execution_outbox::{AdmissionCancel, ControlChildRequest};
use runtara_component_host::control_host::CommandOutcome;
use runtara_core::persistence::control_receipts::{
    BeginReceipt, ControlIntent, ControlReceipt, ControlReceiptState, ControlReceipts,
};
use runtara_core::persistence::inputs::{InputError, InputRequest, InputState};
use runtara_environment::control_reads::{AdmittedChild, ChildOrder, ChildOutcomes, ControlChild};
use sha2::{Digest, Sha256};

/// How long a call waits for the embedded runtime to install the service.
pub const INSTALL_WAIT: Duration = Duration::from_secs(30);

/// Longest instance id, workflow id or filter value control accepts.
const MAX_ID_BYTES: usize = 256;

/// Longest cancel reason control accepts.
const MAX_REASON_BYTES: usize = 1024;

/// How a control command's target relates to the calling instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The caller itself.
    SelfCall,
    /// A direct child of the caller.
    Child,
    /// An ancestor of the caller.
    Ancestor,
    /// Any other run of the tenant.
    Other,
}

/// A control mutation, for [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    SendSignal,
    Cancel,
    Pause,
    Resume,
    /// Admission of a child; authorized by the caller being a run, not by
    /// [`decide`].
    Start,
}

impl Mutation {
    /// Receipt and audit spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::SendSignal => "send_signal",
            Self::Cancel => "cancel",
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}

/// Decision D1, pure. No mutation targets the caller (`invalid`).
/// `send-signal` reaches children and ancestors, and any other run only
/// through a request that opted in with `action.key` (`denied` otherwise).
/// `cancel`, `pause` and `resume` reach direct children only: an ancestor is
/// `denied`, anything else `not-child`.
pub fn decide(
    mutation: Mutation,
    relation: Relation,
    opted_in: bool,
) -> Result<(), ControlErrorCode> {
    match (mutation, relation) {
        (_, Relation::SelfCall) => Err(ControlErrorCode::Invalid),
        (Mutation::Start, _) => Ok(()),
        (Mutation::SendSignal, Relation::Child | Relation::Ancestor) => Ok(()),
        (Mutation::SendSignal, Relation::Other) if opted_in => Ok(()),
        (Mutation::SendSignal, Relation::Other) => Err(ControlErrorCode::Denied),
        (_, Relation::Child) => Ok(()),
        (_, Relation::Ancestor) => Err(ControlErrorCode::Denied),
        (_, Relation::Other) => Err(ControlErrorCode::NotChild),
    }
}

/// Resolves how a target relates to the caller.
#[async_trait::async_trait]
pub trait RelationResolver: Send + Sync {
    async fn relation(
        &self,
        tenant: &str,
        caller: &str,
        target: &str,
    ) -> Result<Relation, ControlError>;
}

/// How `target` relates to a caller whose lineage (itself first) is
/// `lineage`, given the parent `target` records, if any. Pure.
pub fn relate(
    caller: &str,
    target: &str,
    target_parent: Option<&str>,
    lineage: &[(String, Option<String>)],
) -> Relation {
    if caller == target {
        Relation::SelfCall
    } else if target_parent == Some(caller) {
        Relation::Child
    } else if lineage
        .iter()
        .any(|(_, parent)| parent.as_deref() == Some(target))
    {
        Relation::Ancestor
    } else {
        Relation::Other
    }
}

/// The input operation id control answers a request with: `control:` plus
/// the hex sha256 of `caller ‖ 0x1f ‖ op_hash`. The `control:` space is
/// reserved on every public submission path.
pub fn control_operation_id(caller: &str, op_hash: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(caller.as_bytes());
    hash.update([0x1f]);
    hash.update(op_hash.as_bytes());
    format!(
        "{}{:x}",
        crate::runtime_client::CONTROL_OPERATION_PREFIX,
        hash.finalize()
    )
}

/// Fingerprint of a mutation's arguments: `v1:` plus the hex sha256 of their
/// canonical JSON. A replay with another fingerprint is `replay-conflict`.
pub fn fingerprint(mutation: Mutation, arguments: &Value) -> String {
    let canonical = runtara_core::persistence::inputs::canonical_payload(&json!({
        "command": mutation.as_str(),
        "arguments": arguments,
    }));
    format!("v1:{:x}", Sha256::digest(canonical))
}

/// The native control service. Construct once, hand it to the control
/// executor, and [`install`](Self::install) the runtime once it exists.
pub struct NativeControl {
    runtime: tokio::sync::watch::Sender<Option<Arc<RuntimeClient>>>,
    /// The only tenant this process serves; `None` accepts any non-empty
    /// tenant (tests).
    tenant: Option<String>,
    install_wait: Duration,
    /// The execution engine `start` admits children through, and whose
    /// admission records hold children not launched yet.
    engine: tokio::sync::watch::Sender<Option<Arc<ExecutionEngine>>>,
    /// Overrides the native lineage resolution (tests).
    relations: Option<Arc<dyn RelationResolver>>,
    /// The server database `audit_events` lives in, when auditing.
    audit: Option<sqlx::PgPool>,
    /// The instance waits `WaitForInstances` steps call; installs and the
    /// relations reach it too.
    waits: Arc<InstanceWaits>,
}

impl NativeControl {
    /// A service for `tenant` that waits [`INSTALL_WAIT`] for its runtime.
    pub fn new(tenant: Option<String>) -> Self {
        Self::with_install_wait(tenant, INSTALL_WAIT)
    }

    /// [`Self::new`] with a custom install wait.
    pub fn with_install_wait(tenant: Option<String>, install_wait: Duration) -> Self {
        Self {
            runtime: tokio::sync::watch::channel(None).0,
            waits: Arc::new(InstanceWaits::with_install_wait(
                tenant.clone(),
                install_wait,
            )),
            tenant,
            install_wait,
            engine: tokio::sync::watch::channel(None).0,
            relations: None,
            audit: None,
        }
    }

    /// Record every mutation attempt in `audit_events` of `pool`.
    pub fn with_audit(mut self, pool: sqlx::PgPool) -> Self {
        self.audit = Some(pool);
        self
    }

    /// Resolve relations with `relations` instead of the parent links, for
    /// the waits too.
    pub fn with_relations(mut self, relations: Arc<dyn RelationResolver>) -> Self {
        let waits = InstanceWaits::with_install_wait(self.tenant.clone(), self.install_wait)
            .with_relations(relations.clone());
        if let Some(runtime) = self.runtime.borrow().clone() {
            waits.install(runtime);
        }
        if let Some(engine) = self.engine.borrow().clone() {
            waits.install_engine(engine);
        }
        self.waits = Arc::new(waits);
        self.relations = Some(relations);
        self
    }

    /// Bind the embedded runtime; calls waiting for it proceed.
    pub fn install(&self, runtime: Arc<RuntimeClient>) {
        self.waits.install(runtime.clone());
        self.runtime.send_replace(Some(runtime));
    }

    /// Bind the execution engine; `start` calls waiting for it proceed.
    pub fn install_engine(&self, engine: Arc<ExecutionEngine>) {
        self.waits.install_engine(engine.clone());
        self.engine.send_replace(Some(engine));
    }

    /// The instance wait service for `WaitForInstances` steps, bound by
    /// [`Self::install`] and [`Self::install_engine`].
    pub fn instance_waits(&self) -> Arc<InstanceWaits> {
        self.waits.clone()
    }

    /// The engine, waiting for it like [`Self::runtime`].
    async fn engine(&self) -> Result<Arc<ExecutionEngine>, ControlError> {
        let mut receiver = self.engine.subscribe();
        let installed = tokio::time::timeout(
            self.install_wait,
            receiver.wait_for(|engine| engine.is_some()),
        )
        .await;
        match installed {
            Ok(Ok(engine)) => Ok(engine.clone().expect("waited for an installed engine")),
            _ => Err(unavailable("the control service is not ready yet")),
        }
    }

    /// The engine if installed. Reads that merely add children still in
    /// admission do without it rather than wait.
    fn installed_engine(&self) -> Option<Arc<ExecutionEngine>> {
        self.engine.borrow().clone()
    }

    /// The server's admission record of `instance_id`, a child `start`
    /// admitted, if any.
    async fn admitted_child(
        &self,
        tenant: &str,
        instance_id: &str,
    ) -> Result<Option<ControlChildRequest>, ControlError> {
        match self.installed_engine() {
            None => Ok(None),
            Some(engine) => engine
                .control_child(tenant, instance_id)
                .await
                .map_err(|_| unavailable("the run could not be read")),
        }
    }

    /// How `target` relates to `caller`, from the parent links.
    async fn relation(
        &self,
        tenant: &str,
        caller: &str,
        target: &str,
    ) -> Result<Relation, ControlError> {
        if let Some(relations) = &self.relations {
            return relations.relation(tenant, caller, target).await;
        }
        if caller == target {
            return Ok(Relation::SelfCall);
        }
        let runtime = self.runtime().await?;
        let lineage_error = |_| unavailable("the run's lineage could not be read");
        let target_parent = match runtime
            .control_instance(tenant, target, 0, 0)
            .await
            .map_err(lineage_error)?
        {
            Some(row) => row.parent_instance_id,
            None => self
                .admitted_child(tenant, target)
                .await?
                .and_then(|request| request.parent_instance_id),
        };
        if target_parent.as_deref() == Some(caller) {
            return Ok(Relation::Child);
        }
        let lineage = runtime
            .control_lineage(tenant, caller)
            .await
            .map_err(lineage_error)?;
        Ok(relate(caller, target, target_parent.as_deref(), &lineage))
    }

    async fn runtime(&self) -> Result<Arc<RuntimeClient>, ControlError> {
        let mut receiver = self.runtime.subscribe();
        let installed = tokio::time::timeout(
            self.install_wait,
            receiver.wait_for(|runtime| runtime.is_some()),
        )
        .await;
        match installed {
            Ok(Ok(runtime)) => Ok(runtime.clone().expect("waited for an installed runtime")),
            _ => Err(unavailable("the control service is not ready yet")),
        }
    }

    /// The tenant of the call, which must be this process's.
    fn tenant<'a>(&self, authority: &'a ControlAuthority) -> Result<&'a str, ControlError> {
        let tenant = authority.tenant.as_str();
        if tenant.is_empty() || self.tenant.as_deref().is_some_and(|own| own != tenant) {
            return Err(ControlError::new(
                ControlErrorCode::Denied,
                "the call's tenant is not served here",
            ));
        }
        Ok(tenant)
    }
}

/// A mutation needs a calling instance, then an operation scope.
fn mutation_identity<'a>(
    authority: &'a ControlAuthority,
    what: &str,
) -> Result<(&'a str, &'a str), ControlError> {
    let caller = authority.caller.as_deref().ok_or_else(|| {
        ControlError::new(
            ControlErrorCode::RequiresInstance,
            format!("{what} needs a calling run"),
        )
    })?;
    let operation = authority.operation.as_deref().ok_or_else(|| {
        ControlError::new(
            ControlErrorCode::RequiresOperation,
            format!("{what} runs only inside a compiler-emitted operation scope"),
        )
    })?;
    Ok((caller, operation))
}

fn refused(code: ControlErrorCode, mutation: Mutation) -> ControlError {
    let message = match code {
        ControlErrorCode::Invalid => "a control command cannot target the calling run",
        ControlErrorCode::NotChild => "the target is not a direct child of the calling run",
        ControlErrorCode::Denied if mutation == Mutation::SendSignal => {
            "send-signal reaches only children, ancestors, and requests that opt in with action.key"
        }
        _ => "the calling run may not control an ancestor",
    };
    ControlError::new(code, message)
}

fn execution_error(error: ExecutionError) -> ControlError {
    match error {
        ExecutionError::NotFound(_) => not_found(),
        ExecutionError::ValidationError(message) => invalid(message),
        _ => unavailable("the command could not be applied"),
    }
}

fn store_error(_: runtara_core::error::CoreError) -> ControlError {
    unavailable("control receipts are unavailable")
}

/// The WIT spelling of an error code.
fn code_name(code: ControlErrorCode) -> &'static str {
    use ControlErrorCode as C;
    match code {
        C::Denied => "denied",
        C::Invalid => "invalid",
        C::NotFound => "not-found",
        C::NotRunnable => "not-runnable",
        C::NotChild => "not-child",
        C::RequiresInstance => "requires-instance",
        C::RequiresOperation => "requires-operation",
        C::Capacity => "capacity",
        C::ReplayConflict => "replay-conflict",
        C::LabelConflict => "label-conflict",
        C::TooLarge => "too-large",
        C::Unavailable => "unavailable",
        C::Unsupported => "unsupported",
        C::NotWaiting => "not-waiting",
        C::Ambiguous => "ambiguous",
        C::AlreadyAnswered => "already-answered",
        C::NotPausable => "not-pausable",
        C::NotPaused => "not-paused",
    }
}

fn outcome_name(outcome: CommandOutcome) -> &'static str {
    match outcome {
        CommandOutcome::Requested => "requested",
        CommandOutcome::Applied => "applied",
        CommandOutcome::Unchanged => "unchanged",
        CommandOutcome::AlreadyTerminal => "already_terminal",
    }
}

fn outcome_from_name(name: Option<&str>) -> Option<CommandOutcome> {
    Some(match name? {
        "requested" => CommandOutcome::Requested,
        "applied" => CommandOutcome::Applied,
        "unchanged" => CommandOutcome::Unchanged,
        "already_terminal" => CommandOutcome::AlreadyTerminal,
        _ => return None,
    })
}

fn command_outcome(effect: CommandEffect) -> CommandOutcome {
    match effect {
        CommandEffect::Requested => CommandOutcome::Requested,
        CommandEffect::Applied => CommandOutcome::Applied,
        CommandEffect::Unchanged => CommandOutcome::Unchanged,
        CommandEffect::AlreadyTerminal => CommandOutcome::AlreadyTerminal,
    }
}

/// The step id a request answers to, as `list-pending-signals` reports it.
fn request_signal_id(request: &InputRequest) -> &str {
    request
        .spec
        .metadata
        .get("step_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(&request.spec.signal_id)
}

fn request_action_key(request: &InputRequest) -> Option<&str> {
    request
        .spec
        .metadata
        .get("action_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
}

/// Decision D1's opt-in: the request carries an `action.key` and the caller
/// named exactly it.
fn opted_in(request: &InputRequest, action_key: Option<&str>) -> bool {
    request_action_key(request).is_some_and(|key| Some(key) == action_key)
}

fn submission_error(error: InputError) -> ControlError {
    match error {
        InputError::InvalidPayload(_) => {
            invalid("the payload does not match the request's response schema")
        }
        InputError::InvalidRequest => invalid("the signal request is malformed"),
        InputError::AlreadyAnswered => ControlError::new(
            ControlErrorCode::AlreadyAnswered,
            "another operation already answered the request",
        ),
        InputError::Inactive | InputError::NotFound => ControlError::new(
            ControlErrorCode::NotWaiting,
            "the request is no longer open",
        ),
        InputError::OperationConflict => ControlError::new(
            ControlErrorCode::ReplayConflict,
            "the operation already answered with different arguments",
        ),
        _ => unavailable("the signal could not be submitted"),
    }
}

/// What a receipt lookup decided before a mutation applies.
enum Prior {
    /// No receipt yet: apply fresh.
    Fresh,
    /// An intent without an outcome: re-apply it.
    Pending(ControlReceipt),
    /// A completed operation: answer from its result.
    Completed(ControlReceipt),
}

fn prior(receipt: Option<ControlReceipt>, intent: &ControlIntent) -> Result<Prior, ControlError> {
    let Some(receipt) = receipt else {
        return Ok(Prior::Fresh);
    };
    if receipt.intent.command != intent.command
        || receipt.intent.target_instance_id != intent.target_instance_id
        || receipt.intent.fingerprint != intent.fingerprint
    {
        return Err(ControlError::new(
            ControlErrorCode::ReplayConflict,
            "this operation already ran with different arguments",
        ));
    }
    Ok(match receipt.state {
        ControlReceiptState::Pending => Prior::Pending(receipt),
        ControlReceiptState::Completed => Prior::Completed(receipt),
    })
}

impl NativeControl {
    /// Record one mutation attempt. Never carries payloads; best-effort.
    #[allow(clippy::too_many_arguments)]
    async fn audit(
        &self,
        tenant: &str,
        caller: &str,
        operation: &str,
        mutation: Mutation,
        target: &str,
        result: &Result<(String, bool), ControlError>,
    ) {
        let (outcome, replayed, error) = match result {
            Ok((outcome, replayed)) => (Some(outcome.as_str()), *replayed, None),
            Err(error) => (None, false, Some(code_name(error.code).to_owned())),
        };
        tracing::info!(
            tenant,
            caller,
            operation,
            command = mutation.as_str(),
            target,
            outcome,
            replayed,
            error = error.as_deref(),
            "control command"
        );
        if let Some(pool) = &self.audit {
            crate::audit::emit(
                pool,
                tenant,
                None,
                crate::audit::AuditEvent::new(format!("control.{}", mutation.as_str()))
                    .resource("workflow_instance", target)
                    .payload(json!({
                        "callerInstanceId": caller,
                        "operation": operation,
                        "outcome": outcome,
                        "replayed": replayed,
                        "error": error,
                    })),
            )
            .await;
        }
    }

    /// Authorize a lifecycle command, then apply it under its receipt.
    async fn lifecycle(
        &self,
        authority: &ControlAuthority,
        mutation: Mutation,
        instance_id: String,
        arguments: Value,
        cancel: Option<(u64, String)>,
    ) -> Result<CommandResult, ControlError> {
        let tenant = self.tenant(authority)?;
        let (caller, operation) = mutation_identity(authority, mutation.as_str())?;
        check_id("instanceId", &instance_id)?;
        let result = self
            .run_lifecycle(
                tenant,
                caller,
                operation,
                mutation,
                &instance_id,
                arguments,
                cancel,
            )
            .await;
        self.audit(
            tenant,
            caller,
            operation,
            mutation,
            &instance_id,
            &result
                .as_ref()
                .map(|result| (outcome_name(result.outcome).to_owned(), result.replayed))
                .map_err(Clone::clone),
        )
        .await;
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_lifecycle(
        &self,
        tenant: &str,
        caller: &str,
        operation: &str,
        mutation: Mutation,
        instance_id: &str,
        arguments: Value,
        cancel: Option<(u64, String)>,
    ) -> Result<CommandResult, ControlError> {
        let runtime = self.runtime().await?;
        let relation = self.relation(tenant, caller, instance_id).await?;
        decide(mutation, relation, false).map_err(|code| refused(code, mutation))?;
        let receipts = receipts(&runtime)?;
        let intent = ControlIntent {
            command: mutation.as_str().into(),
            target_instance_id: instance_id.into(),
            fingerprint: fingerprint(mutation, &arguments),
            detail: json!({}),
        };
        let begun = receipts
            .begin(caller, operation, &intent)
            .await
            .map_err(store_error)?;
        let reapplying = match begun {
            BeginReceipt::Started(_) => false,
            BeginReceipt::Existing(receipt) => match prior(Some(receipt), &intent)? {
                Prior::Fresh => false,
                Prior::Pending(_) => true,
                Prior::Completed(receipt) => {
                    let outcome = outcome_from_name(
                        receipt
                            .result
                            .as_ref()
                            .and_then(|result| result.get("outcome"))
                            .and_then(Value::as_str),
                    )
                    .ok_or_else(|| unavailable("the stored control receipt is unreadable"))?;
                    return Ok(CommandResult {
                        instance_id: instance_id.into(),
                        outcome,
                        replayed: true,
                    });
                }
            },
        };
        // A run with an instance row is stopped in Environment: the row
        // wins. A child without one is cancelled in admission (a launch that
        // races this is stopped when its outcome is published).
        let launched = mutation == Mutation::Cancel
            && runtime
                .control_instance(tenant, instance_id, 0, 0)
                .await
                .map_err(|_| unavailable("the run could not be read"))?
                .is_some();
        let admission = match (&cancel, self.installed_engine()) {
            (Some((grace_ms, reason)), Some(engine))
                if mutation == Mutation::Cancel && !launched =>
            {
                engine
                    .outbox()
                    .cancel_request(tenant, instance_id, reason, *grace_ms)
                    .await
                    .map_err(|_| unavailable("the child's admission could not be read"))
                    .map(Some)
            }
            _ => Ok(None),
        };
        let applied = match admission {
            Err(error) => Err(error),
            Ok(Some(AdmissionCancel::Cancelled)) => Ok(CommandOutcome::Applied),
            Ok(Some(AdmissionCancel::IntentStored)) => Ok(CommandOutcome::Requested),
            Ok(Some(AdmissionCancel::AlreadyEnded { .. })) => {
                match apply_lifecycle(&runtime, tenant, mutation, instance_id, cancel, reapplying)
                    .await
                {
                    // Never launched: the ended admission is the outcome.
                    Err(error) if error.code == ControlErrorCode::NotFound => {
                        Ok(CommandOutcome::AlreadyTerminal)
                    }
                    other => other,
                }
            }
            Ok(_) => {
                apply_lifecycle(&runtime, tenant, mutation, instance_id, cancel, reapplying).await
            }
        };
        // A child still in admission has no run to pause or resume yet.
        let applied = match applied {
            Err(error)
                if error.code == ControlErrorCode::NotFound
                    && matches!(mutation, Mutation::Pause | Mutation::Resume)
                    && self
                        .admitted_child(tenant, instance_id)
                        .await?
                        .is_some_and(|request| request.in_admission()) =>
            {
                Err(if mutation == Mutation::Pause {
                    ControlError::new(
                        ControlErrorCode::NotPausable,
                        "the child has not started yet",
                    )
                } else {
                    ControlError::new(ControlErrorCode::NotPaused, "the child has not started yet")
                })
            }
            other => other,
        };
        match applied {
            Ok(outcome) => {
                receipts
                    .complete(
                        caller,
                        operation,
                        &json!({ "outcome": outcome_name(outcome) }),
                    )
                    .await
                    .map_err(store_error)?;
                Ok(CommandResult {
                    instance_id: instance_id.into(),
                    outcome,
                    replayed: false,
                })
            }
            Err(error) => {
                if let Err(discard) = receipts.discard(caller, operation).await {
                    tracing::warn!(error = %discard, "control receipt discard failed");
                }
                Err(error)
            }
        }
    }

    async fn run_send_signal(
        &self,
        tenant: &str,
        caller: &str,
        operation: &str,
        request: SendSignalRequest,
        payload: Value,
    ) -> Result<SendSignalResult, ControlError> {
        let runtime = self.runtime().await?;
        let receipts = receipts(&runtime)?;
        let arguments = json!({
            "instanceId": request.instance_id,
            "signalId": request.signal_id,
            "actionKey": request.action_key,
            "requestId": request.request_id,
            "payload": payload,
        });
        let intent = ControlIntent {
            command: Mutation::SendSignal.as_str().into(),
            target_instance_id: request.instance_id.clone(),
            fingerprint: fingerprint(Mutation::SendSignal, &arguments),
            detail: json!({}),
        };
        let relation = self.relation(tenant, caller, &request.instance_id).await?;
        if relation == Relation::SelfCall {
            return Err(refused(ControlErrorCode::Invalid, Mutation::SendSignal));
        }
        let existing = receipts
            .receipt_by_operation(caller, operation)
            .await
            .map_err(store_error)?;
        // A pending intent re-applies to the request it resolved, which may
        // no longer be open because this very operation answered it.
        let request_id = match prior(existing, &intent)? {
            Prior::Completed(receipt) => {
                let request_id = receipt
                    .result
                    .as_ref()
                    .and_then(|result| result.get("requestId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| unavailable("the stored control receipt is unreadable"))?;
                return Ok(SendSignalResult {
                    request_id: request_id.into(),
                    replayed: true,
                });
            }
            Prior::Pending(receipt) => {
                let request_id = receipt
                    .intent
                    .detail
                    .get("requestId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| unavailable("the stored control receipt is unreadable"))?
                    .to_owned();
                let target = runtime
                    .get_input_request(tenant, &request.instance_id, &request_id)
                    .await
                    .map_err(submission_error)?;
                decide(
                    Mutation::SendSignal,
                    relation,
                    opted_in(&target, request.action_key.as_deref()),
                )
                .map_err(|code| refused(code, Mutation::SendSignal))?;
                request_id
            }
            Prior::Fresh => {
                let target = self.open_request(&runtime, tenant, &request).await?;
                decide(
                    Mutation::SendSignal,
                    relation,
                    opted_in(&target, request.action_key.as_deref()),
                )
                .map_err(|code| refused(code, Mutation::SendSignal))?;
                let intent = ControlIntent {
                    detail: json!({ "requestId": target.request_id }),
                    ..intent.clone()
                };
                match receipts
                    .begin(caller, operation, &intent)
                    .await
                    .map_err(store_error)?
                {
                    BeginReceipt::Started(_) => target.request_id,
                    // A concurrent attempt of the same operation won the
                    // intent; this one yields rather than answer twice.
                    BeginReceipt::Existing(_) => {
                        return Err(unavailable(
                            "the operation is being applied concurrently; retry",
                        ));
                    }
                }
            }
        };

        let op_id = control_operation_id(caller, operation);
        let context = runtara_core::persistence::inputs::InputAcceptanceContext::new(
            "control",
            caller,
            &json!({
                "callerInstanceId": caller,
                "operation": operation,
                "instanceId": request.instance_id,
                "signalId": request.signal_id,
            }),
            &payload,
        )
        .map_err(|_| invalid("the signal request is malformed"))?;
        let submitted = match runtime
            .replay_control_input_response(
                tenant,
                &request.instance_id,
                &request_id,
                &op_id,
                &context,
            )
            .await
        {
            Ok(Some(_)) => Ok(true),
            Ok(None) => runtime
                .submit_control_input_response(
                    tenant,
                    &request.instance_id,
                    &request_id,
                    &op_id,
                    &payload,
                    &context,
                )
                .await
                .map(|_| false),
            Err(error) => Err(error),
        };
        match submitted {
            Ok(replayed) => {
                receipts
                    .complete(caller, operation, &json!({ "requestId": request_id }))
                    .await
                    .map_err(store_error)?;
                Ok(SendSignalResult {
                    request_id,
                    replayed,
                })
            }
            Err(error) => {
                if let Err(discard) = receipts.discard(caller, operation).await {
                    tracing::warn!(error = %discard, "control receipt discard failed");
                }
                Err(submission_error(error))
            }
        }
    }

    /// The one open request of the target's `WaitForSignal` step, narrowed by
    /// `actionKey` and `requestId`.
    async fn open_request(
        &self,
        runtime: &RuntimeClient,
        tenant: &str,
        request: &SendSignalRequest,
    ) -> Result<InputRequest, ControlError> {
        runtime
            .control_instance(tenant, &request.instance_id, 0, 0)
            .await
            .map_err(|_| unavailable("the run could not be read"))?
            .ok_or_else(not_found)?;
        let page = runtime
            .list_input_requests(
                tenant,
                std::slice::from_ref(&request.instance_id),
                0,
                u32::MAX,
            )
            .await
            .map_err(input_error)?;
        let mut matching: Vec<InputRequest> = page
            .requests
            .into_iter()
            .filter(|open| {
                request_signal_id(open) == request.signal_id
                    && request
                        .action_key
                        .as_deref()
                        .is_none_or(|key| request_action_key(open) == Some(key))
                    && request
                        .request_id
                        .as_deref()
                        .is_none_or(|id| open.request_id == id)
            })
            .collect();
        match matching.len() {
            1 => Ok(matching.pop().expect("one match")),
            0 => {
                // A named request that is no longer open may have been
                // answered by someone else.
                if let Some(id) = &request.request_id
                    && let Ok(named) = runtime
                        .get_input_request(tenant, &request.instance_id, id)
                        .await
                    && matches!(named.state, InputState::Accepted { .. })
                {
                    return Err(ControlError::new(
                        ControlErrorCode::AlreadyAnswered,
                        "another operation already answered the request",
                    ));
                }
                Err(ControlError::new(
                    ControlErrorCode::NotWaiting,
                    "no open request waits on that signal",
                ))
            }
            _ => Err(ControlError::new(
                ControlErrorCode::Ambiguous,
                "several open requests wait on that signal; pass a requestId from list-pending-signals",
            )),
        }
    }
}

impl NativeControl {
    /// `query` restricted to the children of `parent`: the server's children
    /// still in admission (filtered here) merged with the launched ones in
    /// one runtime read, paged by admission time.
    async fn query_children(
        &self,
        tenant: &str,
        parent: &str,
        request: &QueryRequest,
        statuses: Vec<String>,
        limit: i64,
        offset: u64,
    ) -> Result<InstancePage, ControlError> {
        let created_after = time("createdAfterMs", request.created_after_ms)?;
        let created_before = time("createdBeforeMs", request.created_before_ms)?;
        let finished_after = time("finishedAfterMs", request.finished_after_ms)?;
        let finished_before = time("finishedBeforeMs", request.finished_before_ms)?;
        let include_launched = request.statuses.is_empty() || !statuses.is_empty();
        let include_admitted = (request.statuses.is_empty()
            || request.statuses.contains(&InstanceStatus::Queued))
            && finished_after.is_none()
            && finished_before.is_none();
        let requests: Vec<ControlChildRequest> = match (include_admitted, self.installed_engine()) {
            (true, Some(engine)) => engine
                .admitted_children(tenant, parent)
                .await
                .map_err(|_| unavailable("the children could not be listed"))?
                .into_iter()
                .filter(|child| {
                    request
                        .workflow_id
                        .as_ref()
                        .is_none_or(|workflow| &child.workflow_id == workflow)
                        && request
                            .run_label
                            .as_ref()
                            .is_none_or(|label| child.run_label.as_ref() == Some(label))
                        && created_after.is_none_or(|after| child.created_at >= after)
                        && created_before.is_none_or(|before| child.created_at < before)
                })
                .collect(),
            _ => Vec::new(),
        };
        let admitted: Vec<AdmittedChild> = requests
            .iter()
            .map(|child| AdmittedChild {
                instance_id: child.instance_id.clone(),
                run_label: child.run_label.clone(),
                admitted_at: child.created_at,
            })
            .collect();
        let order = match (request.sort_by, request.order) {
            (SortField::CreatedAt, SortOrder::Ascending) => ChildOrder::AdmittedAsc,
            (SortField::CreatedAt, SortOrder::Descending) => ChildOrder::AdmittedDesc,
            (SortField::FinishedAt, SortOrder::Ascending) => ChildOrder::FinishedAsc,
            (SortField::FinishedAt, SortOrder::Descending) => ChildOrder::FinishedDesc,
        };
        let options = ListInstancesOptions {
            run_label: request.run_label.clone(),
            statuses: (!statuses.is_empty()).then_some(statuses),
            image_name_prefix: request
                .workflow_id
                .as_ref()
                .map(|workflow| format!("{workflow}:")),
            created_after,
            created_before,
            finished_after,
            finished_before,
            limit,
            offset: offset as i64,
            ..Default::default()
        };
        let runtime = self.runtime().await?;
        // Ended admissions publish their outcomes first, so the page has no
        // gap between a child leaving admission and its outcome appearing.
        if let Some(engine) = self.installed_engine() {
            let pending = engine
                .outbox()
                .unpublished_outcomes_of(tenant, parent)
                .await
                .map_err(|_| unavailable("the children could not be listed"))?;
            for pending in pending {
                crate::workers::control_children::settle_outcome(
                    engine.outbox(),
                    &runtime,
                    &pending,
                )
                .await
                .map_err(|_| unavailable("the children could not be listed"))?;
            }
        }
        let mut kinds = Vec::new();
        for (status, kind) in [
            (
                InstanceStatus::NotStarted,
                runtara_core::persistence::ExternalOutcomeKind::NotStarted,
            ),
            (
                InstanceStatus::Cancelled,
                runtara_core::persistence::ExternalOutcomeKind::Cancelled,
            ),
        ] {
            if request.statuses.is_empty() || request.statuses.contains(&status) {
                kinds.push(kind);
            }
        }
        let outcomes = ChildOutcomes {
            kinds,
            workflow_id: request.workflow_id.clone(),
        };
        let (children, total) = runtime
            .control_children(
                tenant,
                parent,
                &options,
                include_launched,
                &admitted,
                &outcomes,
                order,
            )
            .await
            .map_err(|_| unavailable("the children could not be listed"))?;
        let total = total.max(0) as u64;
        let next = offset + children.len() as u64;
        let items = children
            .iter()
            .map(|child| match child {
                ControlChild::Launched(row) => summary(row),
                ControlChild::Admitted(admitted) => requests
                    .iter()
                    .find(|request| request.instance_id == admitted.instance_id)
                    .map(queued_summary)
                    .expect("admitted children come from these requests"),
                ControlChild::Outcome(child) => {
                    outcome_summary(&runtara_core::persistence::ExternalOutcomeRecord {
                        outcome: runtara_core::persistence::ExternalOutcome {
                            instance_id: child.instance_id.clone(),
                            tenant_id: tenant.to_owned(),
                            parent_instance_id: parent.to_owned(),
                            outcome: child.outcome,
                            reason: child.reason.clone(),
                            admitted_at: child.admitted_at,
                            workflow_id: child.workflow_id.clone(),
                            workflow_version: child.workflow_version,
                            run_label: child.run_label.clone(),
                        },
                        published_at: child.published_at,
                    })
                }
            })
            .collect();
        Ok(InstancePage {
            items,
            total,
            next_page_token: (next < total && !children.is_empty()).then(|| next.to_string()),
        })
    }
}

/// The runtime's control receipts; mutations fail closed without them.
fn receipts(runtime: &RuntimeClient) -> Result<&dyn ControlReceipts, ControlError> {
    runtime
        .control_receipts()
        .ok_or_else(|| unavailable("control receipts are unavailable"))
}

/// Apply one lifecycle command to an authorized target. `reapplying` is a
/// pending intent's second application: a resume whose run already left the
/// paused state took effect the first time.
async fn apply_lifecycle(
    runtime: &RuntimeClient,
    tenant: &str,
    mutation: Mutation,
    instance_id: &str,
    cancel: Option<(u64, String)>,
    reapplying: bool,
) -> Result<CommandOutcome, ControlError> {
    let not_paused = || {
        ControlError::new(
            ControlErrorCode::NotPaused,
            "the run is not explicitly paused",
        )
    };
    match mutation {
        Mutation::Cancel => {
            let (grace_ms, reason) = cancel.expect("cancel carries its arguments");
            // The stop grace is whole seconds; round a partial second up.
            let grace_seconds = u32::try_from(grace_ms.div_ceil(1000)).unwrap_or(u32::MAX);
            match execution_engine::stop_for(runtime, tenant, instance_id, grace_seconds, &reason)
                .await
                .map_err(execution_error)?
            {
                StopOutcome::AlreadyStopped { .. } => Ok(CommandOutcome::AlreadyTerminal),
                StopOutcome::Stopped { effect, .. } => Ok(command_outcome(effect)),
            }
        }
        Mutation::Pause => match execution_engine::pause_for(runtime, tenant, instance_id)
            .await
            .map_err(execution_error)?
        {
            PauseOutcome::Paused { effect, .. } => Ok(command_outcome(effect)),
            PauseOutcome::AlreadyPaused => Ok(CommandOutcome::Unchanged),
            PauseOutcome::NotPausable { status } => Err(ControlError::new(
                ControlErrorCode::NotPausable,
                format!("a {status} run cannot pause"),
            )),
        },
        Mutation::Resume => {
            match execution_engine::resume_for(runtime, tenant, instance_id, true)
                .await
                .map_err(execution_error)?
            {
                ResumeOutcome::Resumed { .. } => Ok(CommandOutcome::Applied),
                ResumeOutcome::AlreadyRunning | ResumeOutcome::NotPaused if reapplying => {
                    Ok(CommandOutcome::Applied)
                }
                ResumeOutcome::AlreadyRunning
                | ResumeOutcome::NotPaused
                | ResumeOutcome::NotResumable { .. } => Err(not_paused()),
            }
        }
        Mutation::SendSignal | Mutation::Start => {
            unreachable!("not a lifecycle command")
        }
    }
}

fn unavailable(message: &str) -> ControlError {
    ControlError {
        code: ControlErrorCode::Unavailable,
        message: message.into(),
        retry_after_ms: Some(1_000),
    }
}

fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::new(ControlErrorCode::Invalid, message)
}

fn not_found() -> ControlError {
    ControlError::new(ControlErrorCode::NotFound, "no such run in this tenant")
}

/// Caller-relative filters without a calling instance: `requires-instance`.
fn identity_call(what: &str) -> ControlError {
    ControlError::new(
        ControlErrorCode::RequiresInstance,
        format!("{what} needs a calling run"),
    )
}

fn check_id(field: &str, value: &str) -> Result<(), ControlError> {
    if value.trim().is_empty() || value.len() > MAX_ID_BYTES {
        return Err(invalid(format!("{field} must be 1-{MAX_ID_BYTES} bytes")));
    }
    Ok(())
}

fn check_page_size(page_size: u32) -> Result<i64, ControlError> {
    if !(contract::PAGE_SIZE_MIN..=contract::PAGE_SIZE_MAX).contains(&page_size) {
        return Err(invalid(format!(
            "pageSize must be {}-{}",
            contract::PAGE_SIZE_MIN,
            contract::PAGE_SIZE_MAX
        )));
    }
    Ok(i64::from(page_size))
}

/// Page tokens are opaque to callers; here they are the next offset.
fn page_offset(token: Option<&str>) -> Result<u64, ControlError> {
    match token {
        None => Ok(0),
        Some(token) => token
            .parse::<u64>()
            .ok()
            .filter(|offset| *offset <= i64::MAX as u64)
            .ok_or_else(|| invalid("pageToken is not one this service issued")),
    }
}

fn time(field: &str, ms: Option<u64>) -> Result<Option<DateTime<Utc>>, ControlError> {
    ms.map(|ms| {
        i64::try_from(ms)
            .ok()
            .and_then(DateTime::from_timestamp_millis)
            .ok_or_else(|| invalid(format!("{field} is out of range")))
    })
    .transpose()
}

fn millis(at: DateTime<Utc>) -> u64 {
    at.timestamp_millis().max(0) as u64
}

fn status(status: runtara_core::domain::InstanceStatus) -> InstanceStatus {
    use runtara_core::domain::InstanceStatus as Core;
    match status {
        Core::Pending => InstanceStatus::Pending,
        Core::Running => InstanceStatus::Running,
        Core::Suspended => InstanceStatus::Suspended,
        Core::Completed => InstanceStatus::Completed,
        Core::Failed => InstanceStatus::Failed,
        Core::Cancelled => InstanceStatus::Cancelled,
    }
}

/// The stored statuses a requested control status matches. `queued` and
/// `not-started` describe admission, which no run reaches yet.
fn stored_status(status: InstanceStatus) -> Option<&'static str> {
    match status {
        InstanceStatus::Pending => Some("pending"),
        InstanceStatus::Running => Some("running"),
        InstanceStatus::Suspended => Some("suspended"),
        InstanceStatus::Completed => Some("completed"),
        InstanceStatus::Failed => Some("failed"),
        InstanceStatus::Cancelled => Some("cancelled"),
        InstanceStatus::Queued | InstanceStatus::NotStarted => None,
    }
}

fn suspension_reason(row: &ControlInstance) -> Option<SuspensionReason> {
    if row.status != runtara_core::domain::InstanceStatus::Suspended {
        return None;
    }
    if row.explicitly_paused {
        return Some(SuspensionReason::Paused);
    }
    match row.termination_reason.as_deref() {
        Some("waiting_signal") => Some(SuspensionReason::WaitingSignal),
        Some("waiting_instances") => Some(SuspensionReason::WaitingInstances),
        Some("sleeping") => Some(SuspensionReason::Sleeping),
        Some("shutdown_requested" | "environment_restart") => Some(SuspensionReason::Shutdown),
        _ => None,
    }
}

fn summary(row: &ControlInstance) -> InstanceSummary {
    let (workflow_id, version) = row
        .image_name
        .as_deref()
        .map(crate::workers::runtara_dto::parse_image_id)
        .unwrap_or_default();
    InstanceSummary {
        instance_id: row.instance_id.clone(),
        workflow_id,
        version: u32::try_from(version).ok().filter(|version| *version > 0),
        run_label: row.run_label.clone(),
        parent_instance_id: row.parent_instance_id.clone(),
        status: status(row.status),
        suspension_reason: suspension_reason(row),
        termination_reason: row
            .status
            .is_terminal()
            .then(|| row.termination_reason.clone())
            .flatten(),
        created_at_ms: millis(row.created_at),
        started_at_ms: row.started_at.map(millis),
        finished_at_ms: row.finished_at.map(millis),
    }
}

/// A child `start` admitted that has not launched: `queued`.
fn queued_summary(request: &ControlChildRequest) -> InstanceSummary {
    InstanceSummary {
        instance_id: request.instance_id.clone(),
        workflow_id: request.workflow_id.clone(),
        version: request
            .workflow_version
            .and_then(|version| u32::try_from(version).ok())
            .filter(|version| *version > 0),
        run_label: request.run_label.clone(),
        parent_instance_id: request.parent_instance_id.clone(),
        status: InstanceStatus::Queued,
        suspension_reason: None,
        termination_reason: None,
        created_at_ms: millis(request.created_at),
        started_at_ms: None,
        finished_at_ms: None,
    }
}

/// A child that never launched, from its published outcome: `not-started`
/// or `cancelled`, finished when the outcome was published.
fn outcome_summary(record: &runtara_core::persistence::ExternalOutcomeRecord) -> InstanceSummary {
    let outcome = &record.outcome;
    InstanceSummary {
        instance_id: outcome.instance_id.clone(),
        workflow_id: outcome.workflow_id.clone().unwrap_or_default(),
        version: outcome
            .workflow_version
            .and_then(|version| u32::try_from(version).ok())
            .filter(|version| *version > 0),
        run_label: outcome.run_label.clone(),
        parent_instance_id: Some(outcome.parent_instance_id.clone()),
        status: outcome_status(outcome.outcome),
        suspension_reason: None,
        termination_reason: outcome.reason.clone(),
        created_at_ms: millis(outcome.admitted_at),
        started_at_ms: None,
        finished_at_ms: Some(millis(record.published_at)),
    }
}

fn outcome_status(kind: runtara_core::persistence::ExternalOutcomeKind) -> InstanceStatus {
    match kind {
        runtara_core::persistence::ExternalOutcomeKind::NotStarted => InstanceStatus::NotStarted,
        runtara_core::persistence::ExternalOutcomeKind::Cancelled => InstanceStatus::Cancelled,
    }
}

/// Neither a run, a published outcome nor an admission record: it never
/// existed in this tenant, or retention removed it.
fn no_longer_retained() -> ControlError {
    ControlError::new(
        ControlErrorCode::NotFound,
        "no such run in this tenant, or it is no longer retained",
    )
}

fn no_terminal() -> TerminalResult {
    TerminalResult {
        output: None,
        output_bytes: None,
        output_omitted: false,
        error: None,
        error_omitted: false,
    }
}

/// Map an admission refusal to its control error. A full share gets a
/// jittered 3-8 s retry hint; a limit that can never admit a child none.
fn start_error(error: StartChildError) -> ControlError {
    use ControlErrorCode as C;
    match error {
        StartChildError::Invalid(message) => invalid(message),
        StartChildError::NotFound(message) => ControlError::new(C::NotFound, message),
        StartChildError::NotRunnable(message) => ControlError::new(C::NotRunnable, message),
        StartChildError::LabelConflict => ControlError::new(
            C::LabelConflict,
            "this run already gave that runLabel to another child",
        ),
        StartChildError::ReplayConflict => ControlError::new(
            C::ReplayConflict,
            "this operation already started a child with different arguments",
        ),
        StartChildError::Capacity { retryable: true } => ControlError {
            code: C::Capacity,
            message: "the concurrency limit or control's share of it is full; retry later".into(),
            retry_after_ms: Some(contract::capacity_retry_after_ms(rand::random())),
        },
        StartChildError::Capacity { retryable: false } => ControlError::new(
            C::Capacity,
            "a concurrency limit of at most 1 can never admit a child: the calling run holds the only slot",
        ),
        StartChildError::Denied(message) => ControlError::new(C::Denied, message),
        StartChildError::Unavailable(_) => unavailable("the child could not be admitted"),
    }
}

fn wit_policy(
    policy: runtara_component_host::control_host::ParentClosePolicy,
) -> contract::ParentClosePolicy {
    match policy {
        runtara_component_host::control_host::ParentClosePolicy::Cancel => {
            contract::ParentClosePolicy::Cancel
        }
        runtara_component_host::control_host::ParentClosePolicy::LeaveRunning => {
            contract::ParentClosePolicy::LeaveRunning
        }
    }
}

/// The terminal result of a run read with the `get` caps: values over their
/// cap are omitted and flagged, never truncated.
fn terminal(row: &ControlInstance) -> TerminalResult {
    terminal_capped(row, contract::GET_ERROR_INLINE_BYTES)
}

/// [`terminal`] for a row read with an error cap of `error_cap` bytes.
pub(crate) fn terminal_capped(row: &ControlInstance, error_cap: usize) -> TerminalResult {
    if !row.status.is_terminal() {
        return TerminalResult {
            output: None,
            output_bytes: None,
            output_omitted: false,
            error: None,
            error_omitted: false,
        };
    }
    // The error column is text; control hands out JSON, so a message that is
    // not JSON travels as a JSON string.
    let error = row.error.as_deref().map(|text| {
        serde_json::from_str::<Value>(text)
            .unwrap_or_else(|_| Value::String(text.to_owned()))
            .to_string()
            .into_bytes()
    });
    let error_omitted = row.error_bytes.is_some() && error.is_none();
    // The JSON-string wrapping can grow an error past its inline cap.
    let (error, error_omitted) = match error {
        Some(bytes) if bytes.len() > error_cap => (None, true),
        other => (other, error_omitted),
    };
    TerminalResult {
        output_omitted: row.output_bytes.is_some() && row.output.is_none(),
        output: row.output.clone(),
        output_bytes: row.output_bytes,
        error,
        error_omitted,
    }
}

fn input_error(error: runtara_core::persistence::inputs::InputError) -> ControlError {
    use runtara_core::persistence::inputs::InputError;
    match error {
        InputError::NotFound => not_found(),
        _ => unavailable("pending signals are unavailable"),
    }
}

fn pending_signal(
    request: runtara_core::persistence::inputs::InputRequest,
    workflow_id: &str,
) -> PendingSignal {
    let metadata = &request.spec.metadata;
    let text = |key: &str| {
        metadata
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let context = json!({
        "stepName": metadata.get("step_name").cloned().unwrap_or(Value::Null),
        "context": metadata.get("context").cloned().unwrap_or(Value::Null),
        "correlation": metadata.get("correlation").cloned().unwrap_or(Value::Null),
    });
    PendingSignal {
        instance_id: request.instance_id,
        workflow_id: workflow_id.to_owned(),
        signal_id: text("step_id").unwrap_or(request.spec.signal_id),
        request_id: request.request_id,
        action_key: text("action_key"),
        response_schema: request
            .spec
            .response_schema
            .as_ref()
            .map(|schema| schema.to_string().into_bytes()),
        context: Some(context.to_string().into_bytes()),
        requested_at_ms: millis(request.created_at),
        deadline_ms: request.spec.deadline.map(millis),
    }
}

/// Rough wire size of a pending signal, for the response cap.
fn signal_bytes(signal: &PendingSignal) -> usize {
    signal.instance_id.len()
        + signal.workflow_id.len()
        + signal.signal_id.len()
        + signal.request_id.len()
        + signal.action_key.as_ref().map_or(0, String::len)
        + signal.response_schema.as_ref().map_or(0, Vec::len)
        + signal.context.as_ref().map_or(0, Vec::len)
        + 64
}

#[async_trait::async_trait]
impl ControlHost for NativeControl {
    async fn start(
        &self,
        authority: &ControlAuthority,
        request: StartRequest,
    ) -> Result<StartResult, ControlError> {
        let tenant = self.tenant(authority)?;
        let (caller, operation) = mutation_identity(authority, "start")?;
        let start = execution_engine::normalize_start(
            &request.workflow_id,
            request.version,
            &request.input,
            request.run_label.as_deref(),
            wit_policy(request.parent_close_policy),
        )
        .map_err(start_error)?;
        let result = match self.engine().await {
            Ok(engine) => engine
                .start_child(tenant, caller, operation, &start)
                .await
                .map_err(start_error),
            Err(error) => Err(error),
        };
        let target = result
            .as_ref()
            .map(|child| child.instance_id.clone())
            .unwrap_or_else(|_| start.workflow_id.clone());
        self.audit(
            tenant,
            caller,
            operation,
            Mutation::Start,
            &target,
            &result
                .as_ref()
                .map(|child| ("admitted".to_owned(), child.replayed))
                .map_err(Clone::clone),
        )
        .await;
        let child = result?;
        Ok(StartResult {
            instance_id: child.instance_id,
            workflow_id: child.workflow_id,
            version: u32::try_from(child.version).unwrap_or_default(),
            run_label: child.run_label,
            replayed: child.replayed,
        })
    }

    async fn get(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<InstanceDetail, ControlError> {
        let tenant = self.tenant(authority)?;
        check_id("instanceId", &instance_id)?;
        let runtime = self.runtime().await?;
        let row = runtime
            .control_instance(
                tenant,
                &instance_id,
                contract::GET_OUTPUT_INLINE_BYTES,
                contract::GET_ERROR_INLINE_BYTES,
            )
            .await
            .map_err(|_| unavailable("the run could not be read"))?;
        if let Some(row) = row {
            return Ok(InstanceDetail {
                instance: summary(&row),
                terminal: terminal(&row),
            });
        }
        // A child that never launched: its published outcome, ...
        let read_outcome = |_| unavailable("the run could not be read");
        if let Some(record) = runtime
            .get_external_outcome(tenant, &instance_id)
            .await
            .map_err(read_outcome)?
        {
            return Ok(InstanceDetail {
                instance: outcome_summary(&record),
                terminal: no_terminal(),
            });
        }
        // ... or its admission record: `queued` while in admission, and an
        // ended admission publishes its outcome now, under the launch fence.
        let Some(request) = self.admitted_child(tenant, &instance_id).await? else {
            return Err(no_longer_retained());
        };
        if request.in_admission() {
            return Ok(InstanceDetail {
                instance: queued_summary(&request),
                terminal: no_terminal(),
            });
        }
        let Some(engine) = self.installed_engine() else {
            return Err(unavailable("the run could not be read"));
        };
        let Some(pending) = engine
            .outbox()
            .unpublished_outcomes_of(tenant, request.parent_instance_id.as_deref().unwrap_or(""))
            .await
            .map_err(|_| unavailable("the run could not be read"))?
            .into_iter()
            .find(|pending| pending.child.instance_id == instance_id)
        else {
            return Err(no_longer_retained());
        };
        let settled =
            crate::workers::control_children::settle_outcome(engine.outbox(), &runtime, &pending)
                .await
                .map_err(|_| unavailable("the run's outcome could not be published"))?;
        // The core row wins: a launch that beat the fence is read as a run.
        if settled == crate::workers::control_children::OutcomeSettled::Launched
            && let Some(row) = runtime
                .control_instance(
                    tenant,
                    &instance_id,
                    contract::GET_OUTPUT_INLINE_BYTES,
                    contract::GET_ERROR_INLINE_BYTES,
                )
                .await
                .map_err(|_| unavailable("the run could not be read"))?
        {
            return Ok(InstanceDetail {
                instance: summary(&row),
                terminal: terminal(&row),
            });
        }
        match runtime
            .get_external_outcome(tenant, &instance_id)
            .await
            .map_err(read_outcome)?
        {
            Some(record) => Ok(InstanceDetail {
                instance: outcome_summary(&record),
                terminal: no_terminal(),
            }),
            None => Err(no_longer_retained()),
        }
    }

    async fn query(
        &self,
        authority: &ControlAuthority,
        request: QueryRequest,
    ) -> Result<InstancePage, ControlError> {
        let tenant = self.tenant(authority)?;
        let limit = check_page_size(request.page_size)?;
        let offset = page_offset(request.page_token.as_deref())?;
        let parent = match &request.parent {
            None => None,
            Some(ParentFilter::Caller) => Some(
                authority
                    .caller
                    .clone()
                    .ok_or_else(|| identity_call("a caller filter"))?,
            ),
            Some(ParentFilter::Instance(id)) => {
                check_id("parent", id)?;
                Some(id.clone())
            }
        };
        if let Some(workflow) = &request.workflow_id {
            check_id("workflowId", workflow)?;
        }
        if let Some(label) = &request.run_label
            && runtara_dsl::run_label::normalize_run_label(Some(label)).is_err()
        {
            return Err(invalid("runLabel is not a valid run label"));
        }
        let statuses: Vec<String> = request
            .statuses
            .iter()
            .filter_map(|status| stored_status(*status))
            .map(str::to_owned)
            .collect();
        if let Some(parent) = parent {
            return self
                .query_children(tenant, &parent, &request, statuses, limit, offset)
                .await;
        }
        if !request.statuses.is_empty() && statuses.is_empty() {
            // Only admission states, which no run is in yet.
            return Ok(InstancePage {
                items: Vec::new(),
                total: 0,
                next_page_token: None,
            });
        }
        let order_by = match (request.sort_by, request.order) {
            (SortField::CreatedAt, SortOrder::Ascending) => "created_at_asc",
            (SortField::CreatedAt, SortOrder::Descending) => "created_at_desc",
            (SortField::FinishedAt, SortOrder::Ascending) => "finished_at_asc",
            (SortField::FinishedAt, SortOrder::Descending) => "finished_at_desc",
        };
        let options = ListInstancesOptions {
            tenant_id: Some(tenant.to_owned()),
            run_label: request.run_label.clone(),
            statuses: (!statuses.is_empty()).then_some(statuses),
            image_name_prefix: request
                .workflow_id
                .as_ref()
                .map(|workflow| format!("{workflow}:")),
            created_after: time("createdAfterMs", request.created_after_ms)?,
            created_before: time("createdBeforeMs", request.created_before_ms)?,
            finished_after: time("finishedAfterMs", request.finished_after_ms)?,
            finished_before: time("finishedBeforeMs", request.finished_before_ms)?,
            order_by: Some(order_by.into()),
            limit,
            offset: offset as i64,
            ..Default::default()
        };
        let runtime = self.runtime().await?;
        let (rows, total) = runtime
            .control_instances(&options)
            .await
            .map_err(|_| unavailable("runs could not be listed"))?;
        let total = total.max(0) as u64;
        let next = offset + rows.len() as u64;
        Ok(InstancePage {
            items: rows.iter().map(summary).collect(),
            total,
            next_page_token: (next < total && !rows.is_empty()).then(|| next.to_string()),
        })
    }

    async fn list_pending_signals(
        &self,
        authority: &ControlAuthority,
        request: PendingSignalsRequest,
    ) -> Result<PendingSignalPage, ControlError> {
        let tenant = self.tenant(authority)?;
        let limit = check_page_size(request.page_size)? as u32;
        let offset = page_offset(request.page_token.as_deref())?;
        for (field, value) in [
            ("signalId", request.signal_id.as_deref()),
            ("actionKey", request.action_key.as_deref()),
        ] {
            if let Some(value) = value {
                check_id(field, value)?;
            }
        }
        let runtime = self.runtime().await?;
        let (instances, workflow_id) = match &request.scope {
            SignalScope::Children => {
                let caller = authority
                    .caller
                    .as_deref()
                    .ok_or_else(|| identity_call("a children scope"))?;
                let instances = runtime
                    .parent_input_instances(tenant, caller)
                    .await
                    .map_err(input_error)?;
                // Children may run different workflows; each signal names
                // its own run's.
                (instances, String::new())
            }
            SignalScope::Instance(id) => {
                check_id("instanceId", id)?;
                let row = runtime
                    .control_instance(tenant, id, 0, 0)
                    .await
                    .map_err(|_| unavailable("the run could not be read"))?
                    .ok_or_else(not_found)?;
                (vec![id.clone()], summary(&row).workflow_id)
            }
            SignalScope::Workflow(workflow) => {
                check_id("workflowId", workflow)?;
                let instances = runtime
                    .workflow_input_instances(tenant, workflow)
                    .await
                    .map_err(input_error)?;
                (instances, workflow.clone())
            }
        };
        if instances.is_empty() {
            return Ok(PendingSignalPage {
                items: Vec::new(),
                next_page_token: None,
            });
        }
        let filtered = request.signal_id.is_some() || request.action_key.is_some();
        // Filters apply before pagination, so a filtered scope is read whole.
        let (page_offset, page_limit) = if filtered {
            (0, u32::MAX)
        } else {
            (offset, limit)
        };
        let page = runtime
            .list_input_requests(tenant, &instances, page_offset, page_limit)
            .await
            .map_err(input_error)?;
        let total = page.total_count;
        let mut workflows = std::collections::HashMap::new();
        if matches!(request.scope, SignalScope::Children) {
            for instance in page.requests.iter().map(|request| &request.instance_id) {
                if workflows.contains_key(instance) {
                    continue;
                }
                let workflow = runtime
                    .control_instance(tenant, instance, 0, 0)
                    .await
                    .map_err(|_| unavailable("the run could not be read"))?
                    .map(|row| summary(&row).workflow_id)
                    .unwrap_or_default();
                workflows.insert(instance.clone(), workflow);
            }
        }
        let mut signals: Vec<PendingSignal> = page
            .requests
            .into_iter()
            .map(|request| {
                let workflow = workflows
                    .get(&request.instance_id)
                    .cloned()
                    .unwrap_or_else(|| workflow_id.clone());
                pending_signal(request, &workflow)
            })
            .collect();
        let (items, next) = if filtered {
            signals.retain(|signal| {
                request
                    .signal_id
                    .as_ref()
                    .is_none_or(|id| &signal.signal_id == id)
                    && request
                        .action_key
                        .as_ref()
                        .is_none_or(|key| signal.action_key.as_ref() == Some(key))
            });
            let matched = signals.len() as u64;
            let items: Vec<_> = signals
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect();
            let next = offset + items.len() as u64;
            (items, (next < matched).then_some(next))
        } else {
            let next = offset + signals.len() as u64;
            let more = next < total && !signals.is_empty();
            (signals, more.then_some(next))
        };
        if items.iter().map(signal_bytes).sum::<usize>() > contract::MAX_RESPONSE_BYTES {
            return Err(ControlError::new(
                ControlErrorCode::TooLarge,
                "the page is over the response cap; request a smaller pageSize",
            ));
        }
        Ok(PendingSignalPage {
            items,
            next_page_token: next.map(|next| next.to_string()),
        })
    }

    async fn send_signal(
        &self,
        authority: &ControlAuthority,
        request: SendSignalRequest,
    ) -> Result<SendSignalResult, ControlError> {
        let tenant = self.tenant(authority)?;
        let (caller, operation) = mutation_identity(authority, "send-signal")?;
        check_id("instanceId", &request.instance_id)?;
        check_id("signalId", &request.signal_id)?;
        for (field, value) in [
            ("actionKey", request.action_key.as_deref()),
            ("requestId", request.request_id.as_deref()),
        ] {
            if let Some(value) = value {
                check_id(field, value)?;
            }
        }
        let payload: Value = serde_json::from_slice(&request.payload)
            .map_err(|_| invalid("payload must be JSON"))?;
        let target = request.instance_id.clone();
        let result = self
            .run_send_signal(tenant, caller, operation, request, payload)
            .await;
        self.audit(
            tenant,
            caller,
            operation,
            Mutation::SendSignal,
            &target,
            &result
                .as_ref()
                .map(|result| ("accepted".to_owned(), result.replayed))
                .map_err(Clone::clone),
        )
        .await;
        result
    }

    async fn cancel(
        &self,
        authority: &ControlAuthority,
        request: CancelRequest,
    ) -> Result<CommandResult, ControlError> {
        let grace_ms = contract::cancel_grace_ms(request.grace_ms).ok_or_else(|| {
            invalid(format!(
                "graceMs must be 0-{}",
                contract::MAX_CANCEL_GRACE_MS
            ))
        })?;
        let reason = request
            .reason
            .clone()
            .unwrap_or_else(|| "Cancelled by control".into());
        if reason.len() > MAX_REASON_BYTES || reason.chars().any(char::is_control) {
            return Err(invalid(format!(
                "reason must be at most {MAX_REASON_BYTES} bytes of printable text"
            )));
        }
        let arguments = json!({
            "instanceId": request.instance_id,
            "reason": request.reason,
            "graceMs": grace_ms,
        });
        self.lifecycle(
            authority,
            Mutation::Cancel,
            request.instance_id,
            arguments,
            Some((grace_ms, reason)),
        )
        .await
    }

    async fn pause(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        let arguments = json!({ "instanceId": instance_id });
        self.lifecycle(authority, Mutation::Pause, instance_id, arguments, None)
            .await
    }

    async fn resume(
        &self,
        authority: &ControlAuthority,
        instance_id: String,
    ) -> Result<CommandResult, ControlError> {
        let arguments = json!({ "instanceId": instance_id });
        self.lifecycle(authority, Mutation::Resume, instance_id, arguments, None)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(caller: Option<&str>) -> ControlAuthority {
        ControlAuthority {
            tenant: "tenant".into(),
            caller: caller.map(str::to_owned),
            operation: None,
        }
    }

    fn scoped(caller: Option<&str>, operation: Option<&str>) -> ControlAuthority {
        ControlAuthority {
            tenant: "tenant".into(),
            caller: caller.map(str::to_owned),
            operation: operation.map(str::to_owned),
        }
    }

    #[test]
    fn mutations_need_a_caller_then_an_operation() {
        let code =
            |authority: &ControlAuthority| mutation_identity(authority, "cancel").unwrap_err().code;
        assert_eq!(
            code(&scoped(None, Some("op"))),
            ControlErrorCode::RequiresInstance,
            "requires-instance is checked first"
        );
        assert_eq!(
            code(&scoped(Some("parent"), None)),
            ControlErrorCode::RequiresOperation
        );
        assert_eq!(
            mutation_identity(&scoped(Some("parent"), Some("op")), "cancel").unwrap(),
            ("parent", "op")
        );
    }

    /// Decision D1 across every mutation and relation.
    #[test]
    fn decide_matrix() {
        use ControlErrorCode as E;
        use Mutation as M;
        use Relation as R;
        for mutation in [M::SendSignal, M::Cancel, M::Pause, M::Resume] {
            for opted_in in [false, true] {
                let expect = |relation| match (mutation, relation) {
                    (_, R::SelfCall) => Err(E::Invalid),
                    (M::SendSignal, R::Child | R::Ancestor) => Ok(()),
                    (M::SendSignal, R::Other) if opted_in => Ok(()),
                    (M::SendSignal, R::Other) => Err(E::Denied),
                    (_, R::Child) => Ok(()),
                    (_, R::Ancestor) => Err(E::Denied),
                    (_, R::Other) => Err(E::NotChild),
                };
                for relation in [R::SelfCall, R::Child, R::Ancestor, R::Other] {
                    assert_eq!(
                        decide(mutation, relation, opted_in),
                        expect(relation),
                        "{mutation:?} {relation:?} opted_in={opted_in}"
                    );
                }
            }
        }
        // The opt-in never widens the lifecycle commands.
        assert_eq!(decide(M::Cancel, R::Other, true), Err(E::NotChild));
    }

    #[test]
    fn relations_follow_the_parent_links() {
        let lineage = |ids: &[(&str, Option<&str>)]| {
            ids.iter()
                .map(|(id, parent)| (id.to_string(), parent.map(str::to_owned)))
                .collect::<Vec<_>>()
        };
        // me -> parent -> root
        let mine = lineage(&[
            ("me", Some("parent")),
            ("parent", Some("root")),
            ("root", None),
        ]);
        assert_eq!(relate("me", "me", None, &mine), Relation::SelfCall);
        assert_eq!(relate("me", "kid", Some("me"), &mine), Relation::Child);
        assert_eq!(
            relate("me", "parent", Some("root"), &mine),
            Relation::Ancestor
        );
        assert_eq!(relate("me", "root", None, &mine), Relation::Ancestor);
        // A sibling, a grandchild and an unrelated run are `other`.
        assert_eq!(
            relate("me", "sibling", Some("parent"), &mine),
            Relation::Other
        );
        assert_eq!(
            relate("me", "grandkid", Some("kid"), &mine),
            Relation::Other
        );
        assert_eq!(relate("me", "stranger", None, &mine), Relation::Other);
        // An ancestor whose row is gone is still an ancestor by its id.
        let orphan = lineage(&[("me", Some("gone"))]);
        assert_eq!(relate("me", "gone", None, &orphan), Relation::Ancestor);
    }

    #[test]
    fn start_errors_map_to_control_codes() {
        use ControlErrorCode as C;
        let capacity = start_error(StartChildError::Capacity { retryable: true });
        assert_eq!(capacity.code, C::Capacity);
        let hint = capacity
            .retry_after_ms
            .expect("a retryable capacity error hints");
        assert!(
            (contract::CAPACITY_RETRY_MIN_MS..=contract::CAPACITY_RETRY_MAX_MS).contains(&hint)
        );
        let never = start_error(StartChildError::Capacity { retryable: false });
        assert_eq!((never.code, never.retry_after_ms), (C::Capacity, None));
        assert_eq!(
            contract::ErrorCode::Capacity.agent_code(never.retry_after_ms),
            contract::CONTROL_CAPACITY_UNSATISFIABLE
        );
        for (error, code) in [
            (StartChildError::Invalid("x".into()), C::Invalid),
            (StartChildError::NotFound("x".into()), C::NotFound),
            (StartChildError::NotRunnable("x".into()), C::NotRunnable),
            (StartChildError::LabelConflict, C::LabelConflict),
            (StartChildError::ReplayConflict, C::ReplayConflict),
            (StartChildError::Denied("x".into()), C::Denied),
            (StartChildError::Unavailable("db".into()), C::Unavailable),
        ] {
            assert_eq!(start_error(error).code, code);
        }
        // Platform detail never reaches the caller.
        assert!(
            !start_error(StartChildError::Unavailable("secret dsn".into()))
                .message
                .contains("secret")
        );
    }

    #[test]
    fn control_operation_ids_are_reserved_and_bounded() {
        let id = control_operation_id("caller", &"a".repeat(64));
        assert!(crate::runtime_client::is_reserved_operation_id(&id));
        assert_eq!(id.len(), "control:".len() + 64);
        assert!(runtara_core::persistence::inputs::validate_operation_id(&id).is_ok());
        // The separator keeps `(caller, op)` pairs apart.
        assert_ne!(
            control_operation_id("ab", "c"),
            control_operation_id("a", "bc")
        );
        assert_ne!(
            control_operation_id("caller", "op-1"),
            control_operation_id("caller", "op-2")
        );
        for public in ["report-action-1", "controls:1", "Control:1", ""] {
            assert!(!crate::runtime_client::is_reserved_operation_id(public));
        }
        assert!(crate::runtime_client::is_reserved_operation_id("control:x"));
    }

    #[test]
    fn fingerprints_follow_arguments_not_key_order() {
        let a = fingerprint(
            Mutation::Cancel,
            &json!({"instanceId": "c", "graceMs": 5000}),
        );
        let b = fingerprint(
            Mutation::Cancel,
            &json!({"graceMs": 5000, "instanceId": "c"}),
        );
        assert_eq!(a, b);
        assert!(a.starts_with("v1:"));
        assert_ne!(
            a,
            fingerprint(Mutation::Cancel, &json!({"instanceId": "c", "graceMs": 0}))
        );
        assert_ne!(
            a,
            fingerprint(
                Mutation::Pause,
                &json!({"instanceId": "c", "graceMs": 5000})
            )
        );
    }

    #[test]
    fn a_replay_with_other_arguments_conflicts() {
        let intent = ControlIntent {
            command: "pause".into(),
            target_instance_id: "child".into(),
            fingerprint: "v1:a".into(),
            detail: json!({}),
        };
        let receipt = |fingerprint: &str, state| ControlReceipt {
            caller_instance_id: "caller".into(),
            operation_id: "op".into(),
            intent: ControlIntent {
                fingerprint: fingerprint.into(),
                ..intent.clone()
            },
            state,
            result: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
        };
        assert!(matches!(prior(None, &intent), Ok(Prior::Fresh)));
        assert!(matches!(
            prior(Some(receipt("v1:a", ControlReceiptState::Pending)), &intent),
            Ok(Prior::Pending(_))
        ));
        assert!(matches!(
            prior(
                Some(receipt("v1:a", ControlReceiptState::Completed)),
                &intent
            ),
            Ok(Prior::Completed(_))
        ));
        assert_eq!(
            prior(
                Some(receipt("v1:b", ControlReceiptState::Completed)),
                &intent
            )
            .err()
            .unwrap()
            .code,
            ControlErrorCode::ReplayConflict
        );
    }

    #[test]
    fn caller_filters_need_a_caller() {
        assert_eq!(
            identity_call("a caller filter").code,
            ControlErrorCode::RequiresInstance
        );
    }

    #[test]
    fn page_sizes_and_tokens_are_validated() {
        assert!(check_page_size(0).is_err());
        assert!(check_page_size(101).is_err());
        assert_eq!(check_page_size(100).unwrap(), 100);
        assert_eq!(page_offset(None).unwrap(), 0);
        assert_eq!(page_offset(Some("40")).unwrap(), 40);
        assert!(page_offset(Some("next")).is_err());
    }

    #[tokio::test]
    async fn an_uninstalled_service_is_unavailable_and_foreign_tenants_denied() {
        let control = NativeControl::with_install_wait(Some("tenant".into()), Duration::ZERO);
        let error = control
            .get(&authority(None), "run-1".into())
            .await
            .unwrap_err();
        assert_eq!(error.code, ControlErrorCode::Unavailable);
        let mut foreign = authority(None);
        foreign.tenant = "other".into();
        let error = control.get(&foreign, "run-1".into()).await.unwrap_err();
        assert_eq!(error.code, ControlErrorCode::Denied);
    }
}
