//! Durable instance waits behind [`InstanceWaitHost`]: a run waiting on its
//! direct children.
//!
//! The caller (tenant and waiting run) comes from the host, never from
//! arguments, and the wait id is host-derived and scoped to the caller.
//!
//! - Registering checks, in order: the target cap (over 1000 distinct ids is
//!   `too-large`), the ids and the deadline (`invalid`), then a replay: a
//!   wait already registered under the id with the same targets and mode is
//!   found again (its first deadline stands), one with others is
//!   `replay-conflict`. A new wait authorizes every target before anything
//!   registers (decision D1): the caller itself is `invalid`, an unknown id
//!   `not-found`, an ancestor `denied`, any other non-child `not-child`.
//!   Children whose admission ended without a launch get their fenced
//!   outcome first (`unavailable` if that fails), so they read `not-started`
//!   or `cancelled`.
//! - Reading evaluates the wait now. A registered target whose rows are gone
//!   is an explicit `not-found`, never a silent outcome. Finished targets
//!   carry their results under the wait caps (256 KiB output and 16 KiB
//!   error per target, 3 MiB across the wait, spent in finish order), so
//!   every read of a settled wait inlines the same values.
//!
//! Like the control service, it is late-bound: a call waits up to
//! [`INSTALL_WAIT`] for [`InstanceWaits::install`] and is `unavailable`
//! after that. The execution engine ([`InstanceWaits::install_engine`]) is
//! optional; without it no child is in admission.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use runtara_component_host::control_host::ControlError;
use runtara_component_host::instance_wait_host::{
    InstanceWaitAuthority, InstanceWaitError, InstanceWaitErrorCode, InstanceWaitHost,
    InstanceWaitMode, InstanceWaitOutcome, InstanceWaitPoll, InstanceWaitRequest,
    InstanceWaitResolution, InstanceWaitStatus,
};
use runtara_core::persistence::waits::{self as store, TargetState, WaitError, WaitSpec, WaitView};
use runtara_environment::control_reads::ControlInstance;

use super::control::{Relation, RelationResolver, relate};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::ExecutionEngine;
use crate::workers::execution_outbox::ControlChildRequest;

/// How long a call waits for the embedded runtime to install the service.
pub const INSTALL_WAIT: Duration = super::control::INSTALL_WAIT;

/// Longest instance id or wait id accepted.
const MAX_ID_BYTES: usize = 256;

/// Most distinct targets one wait accepts; more is `too-large`.
pub const MAX_WAIT_TARGETS: usize = store::MAX_WAIT_TARGETS;
/// A wait inlines each target's output up to this size.
pub const WAIT_OUTPUT_INLINE_BYTES: usize = 256 * 1024;
/// A wait inlines each target's error up to this size.
pub const WAIT_ERROR_INLINE_BYTES: usize = 16 * 1024;
/// A wait inlines at most this much across all targets.
pub const WAIT_TOTAL_INLINE_BYTES: usize = 3 * 1024 * 1024;

/// Whether registering found an existing wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    /// The wait registered now.
    Registered,
    /// The wait was registered already, with the same targets and mode.
    Replayed,
}

/// The durable instance wait service. Construct once, install the runtime
/// (and the engine) once they exist, and hand it to the workflow runner.
pub struct InstanceWaits {
    runtime: tokio::sync::watch::Sender<Option<Arc<RuntimeClient>>>,
    /// Holds the admission records of children not launched yet.
    engine: tokio::sync::watch::Sender<Option<Arc<ExecutionEngine>>>,
    /// The only tenant this process serves; `None` accepts any non-empty
    /// tenant (tests).
    tenant: Option<String>,
    install_wait: Duration,
    /// Overrides the native lineage resolution (tests).
    relations: Option<Arc<dyn RelationResolver>>,
}

impl InstanceWaits {
    /// A service for `tenant` that waits [`INSTALL_WAIT`] for its runtime.
    pub fn new(tenant: Option<String>) -> Self {
        Self::with_install_wait(tenant, INSTALL_WAIT)
    }

    /// [`Self::new`] with a custom install wait.
    pub fn with_install_wait(tenant: Option<String>, install_wait: Duration) -> Self {
        Self {
            runtime: tokio::sync::watch::channel(None).0,
            engine: tokio::sync::watch::channel(None).0,
            tenant,
            install_wait,
            relations: None,
        }
    }

    /// Resolve relations with `relations` instead of the parent links.
    pub fn with_relations(mut self, relations: Arc<dyn RelationResolver>) -> Self {
        self.relations = Some(relations);
        self
    }

    /// Bind the embedded runtime; calls waiting for it proceed.
    pub fn install(&self, runtime: Arc<RuntimeClient>) {
        self.runtime.send_replace(Some(runtime));
    }

    /// Bind the execution engine, whose admission records hold children not
    /// launched yet.
    pub fn install_engine(&self, engine: Arc<ExecutionEngine>) {
        self.engine.send_replace(Some(engine));
    }

    async fn runtime(&self) -> Result<Arc<RuntimeClient>, InstanceWaitError> {
        let mut receiver = self.runtime.subscribe();
        let installed = tokio::time::timeout(
            self.install_wait,
            receiver.wait_for(|runtime| runtime.is_some()),
        )
        .await;
        match installed {
            Ok(Ok(runtime)) => Ok(runtime.clone().expect("waited for an installed runtime")),
            _ => Err(InstanceWaitError::unavailable(
                "the wait service is not ready yet",
            )),
        }
    }

    fn installed_engine(&self) -> Option<Arc<ExecutionEngine>> {
        self.engine.borrow().clone()
    }

    /// The server's admission record of `instance_id`, a child `start`
    /// admitted, if any.
    async fn admitted_child(
        &self,
        tenant: &str,
        instance_id: &str,
    ) -> Result<Option<ControlChildRequest>, InstanceWaitError> {
        match self.installed_engine() {
            None => Ok(None),
            Some(engine) => engine
                .control_child(tenant, instance_id)
                .await
                .map_err(|_| InstanceWaitError::unavailable("the run could not be read")),
        }
    }

    /// The tenant of the call, which must be this process's.
    fn tenant<'a>(
        &self,
        authority: &'a InstanceWaitAuthority,
    ) -> Result<&'a str, InstanceWaitError> {
        let tenant = authority.tenant.as_str();
        if tenant.is_empty() || self.tenant.as_deref().is_some_and(|own| own != tenant) {
            return Err(InstanceWaitError::new(
                InstanceWaitErrorCode::Denied,
                "the call's tenant is not served here",
            ));
        }
        Ok(tenant)
    }

    /// Validate, find a replay of `wait_id`, authorize every target, then
    /// register. Does not evaluate the wait.
    pub async fn register_wait(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
        request: InstanceWaitRequest,
    ) -> Result<Registration, InstanceWaitError> {
        let tenant = self.tenant(authority)?;
        let caller = authority.caller.as_str();
        let spec = wait_spec(request)?;
        check_id("waitId", wait_id)?;
        let runtime = self.runtime().await?;
        // A replay: the wait is registered already.
        match runtime.poll_instance_wait(tenant, caller, wait_id).await {
            Ok(view) if view.record.fingerprint == spec.fingerprint() => {
                return Ok(Registration::Replayed);
            }
            Ok(_) => return Err(wait_error(WaitError::Conflict)),
            Err(WaitError::NotFound | WaitError::Closed | WaitError::TargetGone(_)) => {}
            Err(error) => return Err(wait_error(error)),
        }
        self.authorize_targets(&runtime, tenant, caller, spec.targets())
            .await?;
        runtime
            .register_instance_wait(tenant, caller, wait_id, &spec)
            .await
            .map_err(wait_error)?;
        Ok(Registration::Registered)
    }

    /// Evaluate the caller's wait `wait_id` now, with its finished targets'
    /// results under the wait caps.
    pub async fn poll_wait(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
    ) -> Result<InstanceWaitPoll, InstanceWaitError> {
        let tenant = self.tenant(authority)?;
        check_id("waitId", wait_id)?;
        let runtime = self.runtime().await?;
        let view = self
            .current_wait(&runtime, tenant, &authority.caller, wait_id)
            .await?;
        read_wait(&runtime, tenant, view).await
    }

    /// Every target must exist in the tenant and be a direct child of the
    /// caller. Children whose admission ended without a launch get their
    /// fenced outcome first, so the wait reads them as finished.
    async fn authorize_targets(
        &self,
        runtime: &RuntimeClient,
        tenant: &str,
        caller: &str,
        targets: &[String],
    ) -> Result<(), InstanceWaitError> {
        if targets.iter().any(|target| target == caller) {
            return Err(refused(InstanceWaitErrorCode::Invalid));
        }
        let read = |_| InstanceWaitError::unavailable("the wait's targets could not be read");
        let mut parents: HashMap<String, Option<String>> = runtime
            .wait_target_statuses(tenant, targets)
            .await
            .map_err(read)?
            .into_iter()
            .map(|target| (target.instance_id, target.parent_instance_id))
            .collect();
        let unknown: Vec<&String> = targets
            .iter()
            .filter(|target| !parents.contains_key(*target))
            .collect();
        if !unknown.is_empty() {
            self.settle_admission_outcomes(runtime, tenant, caller)
                .await?;
            for target in unknown {
                // Still in admission, or ended with an outcome published now.
                let parent = match self.admitted_child(tenant, target).await? {
                    Some(request) => request.parent_instance_id,
                    None => {
                        return Err(InstanceWaitError::new(
                            InstanceWaitErrorCode::NotFound,
                            "no such run in this tenant",
                        ));
                    }
                };
                parents.insert(target.clone(), parent);
            }
        }
        let mut lineage: Option<Vec<(String, Option<String>)>> = None;
        for target in targets {
            let parent = parents.get(target).cloned().flatten();
            let relation = if parent.as_deref() == Some(caller) {
                Relation::Child
            } else if let Some(relations) = &self.relations {
                relations
                    .relation(tenant, caller, target)
                    .await
                    .map_err(relation_error)?
            } else {
                if lineage.is_none() {
                    lineage =
                        Some(runtime.control_lineage(tenant, caller).await.map_err(|_| {
                            InstanceWaitError::unavailable("the run's lineage could not be read")
                        })?);
                }
                relate(
                    caller,
                    target,
                    parent.as_deref(),
                    lineage.as_deref().unwrap_or_default(),
                )
            };
            authorize(relation).map_err(refused)?;
        }
        Ok(())
    }

    /// Publish, under the launch fence, the outcomes of the caller's children
    /// whose admission ended without a launch. A failure is `unavailable`.
    async fn settle_admission_outcomes(
        &self,
        runtime: &RuntimeClient,
        tenant: &str,
        caller: &str,
    ) -> Result<(), InstanceWaitError> {
        let Some(engine) = self.installed_engine() else {
            return Ok(());
        };
        let failed =
            || InstanceWaitError::unavailable("the children's outcomes could not be published");
        for pending in engine
            .outbox()
            .unpublished_outcomes_of(tenant, caller)
            .await
            .map_err(|_| failed())?
        {
            crate::workers::control_children::settle_outcome(engine.outbox(), runtime, &pending)
                .await
                .map_err(|_| failed())?;
        }
        Ok(())
    }

    /// The caller's wait, evaluated now. A target the runtime has no rows
    /// for is a child still in admission, one whose ended admission gets its
    /// outcome published here, or one whose rows are gone: an explicit
    /// `not-found`, never a silent outcome.
    async fn current_wait(
        &self,
        runtime: &RuntimeClient,
        tenant: &str,
        caller: &str,
        wait_id: &str,
    ) -> Result<WaitView, InstanceWaitError> {
        let mut settled = false;
        loop {
            let view = runtime
                .poll_instance_wait(tenant, caller, wait_id)
                .await
                .map_err(wait_error)?;
            let unknown: Vec<&str> = view
                .remaining
                .iter()
                .filter(|target| target.state == TargetState::Unknown)
                .map(|target| target.instance_id.as_str())
                .collect();
            if unknown.is_empty() {
                return Ok(view);
            }
            let mut ended = false;
            for target in unknown {
                match self.admitted_child(tenant, target).await? {
                    Some(request) if request.in_admission() => {}
                    Some(request) if !settled && request.outcome.is_some() => ended = true,
                    _ => return Err(target_gone(target)),
                }
            }
            if !ended {
                return Ok(view);
            }
            self.settle_admission_outcomes(runtime, tenant, caller)
                .await?;
            settled = true;
        }
    }
}

#[async_trait::async_trait]
impl InstanceWaitHost for InstanceWaits {
    async fn register(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
        request: InstanceWaitRequest,
    ) -> Result<InstanceWaitPoll, InstanceWaitError> {
        self.register_wait(authority, wait_id, request).await?;
        self.poll_wait(authority, wait_id).await
    }

    async fn poll(
        &self,
        authority: &InstanceWaitAuthority,
        wait_id: &str,
    ) -> Result<InstanceWaitPoll, InstanceWaitError> {
        self.poll_wait(authority, wait_id).await
    }
}

/// Decision D1 for waits, pure: direct children only. The caller itself is
/// `invalid`, an ancestor `denied`, anything else `not-child`.
pub fn authorize(relation: Relation) -> Result<(), InstanceWaitErrorCode> {
    match relation {
        Relation::SelfCall => Err(InstanceWaitErrorCode::Invalid),
        Relation::Child => Ok(()),
        Relation::Ancestor => Err(InstanceWaitErrorCode::Denied),
        Relation::Other => Err(InstanceWaitErrorCode::NotChild),
    }
}

fn refused(code: InstanceWaitErrorCode) -> InstanceWaitError {
    let message = match code {
        InstanceWaitErrorCode::Invalid => "a run cannot wait on itself",
        InstanceWaitErrorCode::NotChild => "the target is not a direct child of the calling run",
        _ => "the calling run may not wait on an ancestor",
    };
    InstanceWaitError::new(code, message)
}

/// A relation override's failure, under the same code where one exists.
fn relation_error(error: ControlError) -> InstanceWaitError {
    use runtara_component_host::control_host::ControlErrorCode as C;
    let code = match error.code {
        C::Invalid => InstanceWaitErrorCode::Invalid,
        C::Denied => InstanceWaitErrorCode::Denied,
        C::NotChild => InstanceWaitErrorCode::NotChild,
        C::NotFound => InstanceWaitErrorCode::NotFound,
        _ => InstanceWaitErrorCode::Unavailable,
    };
    InstanceWaitError {
        code,
        message: error.message,
        retry_after_ms: error.retry_after_ms,
    }
}

fn invalid(message: impl Into<String>) -> InstanceWaitError {
    InstanceWaitError::new(InstanceWaitErrorCode::Invalid, message)
}

fn check_id(field: &str, value: &str) -> Result<(), InstanceWaitError> {
    if value.trim().is_empty() || value.len() > MAX_ID_BYTES {
        return Err(invalid(format!("{field} must be 1-{MAX_ID_BYTES} bytes")));
    }
    Ok(())
}

/// The stored spec of a request: distinct, sorted targets under the cap,
/// each a valid id, and a deadline in range.
fn wait_spec(request: InstanceWaitRequest) -> Result<WaitSpec, InstanceWaitError> {
    let mut ids = request.instance_ids;
    ids.sort();
    ids.dedup();
    if ids.len() > MAX_WAIT_TARGETS {
        return Err(wait_error(WaitError::TooLarge));
    }
    for id in &ids {
        check_id("instanceIds", id)?;
    }
    let deadline = request
        .deadline_ms
        .map(|ms| {
            i64::try_from(ms)
                .ok()
                .and_then(DateTime::from_timestamp_millis)
                .ok_or_else(|| invalid("deadlineMs is out of range"))
        })
        .transpose()?;
    let mode = match request.mode {
        InstanceWaitMode::All => store::WaitMode::All,
        InstanceWaitMode::Any => store::WaitMode::Any,
    };
    Ok(WaitSpec::new(ids, mode, deadline))
}

fn target_gone(target: &str) -> InstanceWaitError {
    InstanceWaitError::new(
        InstanceWaitErrorCode::NotFound,
        format!("target {target} of this wait is no longer retained"),
    )
}

fn wait_error(error: WaitError) -> InstanceWaitError {
    use InstanceWaitErrorCode as C;
    match error {
        WaitError::NotFound => InstanceWaitError::new(C::NotFound, "no wait is registered here"),
        WaitError::Closed => InstanceWaitError::new(C::Closed, "the wait was closed or released"),
        WaitError::Conflict => InstanceWaitError::new(
            C::ReplayConflict,
            "this wait is registered on other children or in another mode",
        ),
        WaitError::TooLarge => InstanceWaitError::new(
            C::TooLarge,
            format!(
                "a wait names at most {} distinct children",
                MAX_WAIT_TARGETS
            ),
        ),
        WaitError::Invalid(message) => invalid(message),
        WaitError::Inactive => invalid("the calling run has finished"),
        WaitError::TargetGone(target) => target_gone(&target),
        WaitError::Storage(_) => InstanceWaitError::unavailable("instance waits are unavailable"),
    }
}

fn millis(at: DateTime<Utc>) -> u64 {
    at.timestamp_millis().max(0) as u64
}

fn instance_status(status: runtara_core::domain::InstanceStatus) -> InstanceWaitStatus {
    use runtara_core::domain::InstanceStatus as Core;
    match status {
        Core::Pending => InstanceWaitStatus::Pending,
        Core::Running => InstanceWaitStatus::Running,
        Core::Suspended => InstanceWaitStatus::Suspended,
        Core::Completed => InstanceWaitStatus::Completed,
        Core::Failed => InstanceWaitStatus::Failed,
        Core::Cancelled => InstanceWaitStatus::Cancelled,
    }
}

fn outcome_status(kind: runtara_core::persistence::ExternalOutcomeKind) -> InstanceWaitStatus {
    match kind {
        runtara_core::persistence::ExternalOutcomeKind::NotStarted => {
            InstanceWaitStatus::NotStarted
        }
        runtara_core::persistence::ExternalOutcomeKind::Cancelled => InstanceWaitStatus::Cancelled,
    }
}

/// Spend `budget` on one inlined value: over the remaining budget it is
/// omitted and flagged, otherwise it is kept and paid for. A kept value
/// that is not JSON is omitted too (after paying, so the selection matches
/// what the bytes cost).
fn inline_within(
    budget: &mut usize,
    value: Option<Vec<u8>>,
    omitted: bool,
) -> (Option<Vec<u8>>, bool) {
    match value {
        Some(bytes) if bytes.len() > *budget => (None, true),
        Some(bytes) => {
            *budget -= bytes.len();
            if serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).is_ok() {
                (Some(bytes), omitted)
            } else {
                (None, true)
            }
        }
        None => (None, omitted),
    }
}

/// A launched target's outcome, read with the per-target caps and paid
/// from the wait's budget: output first, then error.
fn launched_outcome(row: &ControlInstance, budget: &mut usize) -> InstanceWaitOutcome {
    let result = super::control::terminal_capped(row, WAIT_ERROR_INLINE_BYTES);
    let (output, output_omitted) = inline_within(budget, result.output, result.output_omitted);
    let (error, error_omitted) = inline_within(budget, result.error, result.error_omitted);
    InstanceWaitOutcome {
        instance_id: row.instance_id.clone(),
        status: instance_status(row.status),
        finished_at_ms: row.finished_at.map(millis),
        output,
        output_bytes: result.output_bytes,
        output_omitted,
        error,
        error_omitted,
    }
}

/// A wait read: finished targets in finish order with their results under
/// the wait caps (256 KiB output and 16 KiB error per target, 3 MiB across
/// the wait, in that order, so a repeated read inlines the same values),
/// the remaining ids, and the persisted deadline.
async fn read_wait(
    runtime: &RuntimeClient,
    tenant: &str,
    view: WaitView,
) -> Result<InstanceWaitPoll, InstanceWaitError> {
    let launched: Vec<String> = view
        .finished
        .iter()
        .filter(|target| matches!(target.state, TargetState::Instance { .. }))
        .map(|target| target.instance_id.clone())
        .collect();
    let mut rows: HashMap<String, ControlInstance> = runtime
        .control_instances_by_id(
            tenant,
            &launched,
            WAIT_OUTPUT_INLINE_BYTES,
            WAIT_ERROR_INLINE_BYTES,
        )
        .await
        .map_err(|_| InstanceWaitError::unavailable("the wait's results could not be read"))?
        .into_iter()
        .map(|row| (row.instance_id.clone(), row))
        .collect();
    let mut budget = WAIT_TOTAL_INLINE_BYTES;
    let mut finished = Vec::with_capacity(view.finished.len());
    for target in &view.finished {
        finished.push(match &target.state {
            TargetState::Outcome {
                outcome,
                published_at,
            } => InstanceWaitOutcome {
                instance_id: target.instance_id.clone(),
                status: outcome_status(*outcome),
                finished_at_ms: Some(millis(*published_at)),
                output: None,
                output_bytes: None,
                output_omitted: false,
                error: None,
                error_omitted: false,
            },
            _ => {
                let row = rows
                    .remove(&target.instance_id)
                    .ok_or_else(|| target_gone(&target.instance_id))?;
                launched_outcome(&row, &mut budget)
            }
        });
    }
    Ok(InstanceWaitPoll {
        mode: match view.record.mode {
            store::WaitMode::All => InstanceWaitMode::All,
            store::WaitMode::Any => InstanceWaitMode::Any,
        },
        resolution: view.resolution().map(|resolution| match resolution {
            store::WaitResolution::Satisfied => InstanceWaitResolution::Satisfied,
            store::WaitResolution::Deadline => InstanceWaitResolution::Deadline,
            store::WaitResolution::Empty => InstanceWaitResolution::Empty,
        }),
        finished,
        remaining: view
            .remaining
            .iter()
            .map(|target| target.instance_id.clone())
            .collect(),
        deadline_ms: view.record.deadline.map(millis),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(tenant: &str) -> InstanceWaitAuthority {
        InstanceWaitAuthority {
            tenant: tenant.into(),
            caller: "parent".into(),
        }
    }

    fn request(ids: Vec<String>) -> InstanceWaitRequest {
        InstanceWaitRequest {
            instance_ids: ids,
            mode: InstanceWaitMode::All,
            deadline_ms: None,
        }
    }

    #[test]
    fn wait_errors_map_to_wait_codes() {
        use InstanceWaitErrorCode as C;
        for (error, code) in [
            (WaitError::NotFound, C::NotFound),
            (WaitError::Closed, C::Closed),
            (WaitError::Conflict, C::ReplayConflict),
            (WaitError::TooLarge, C::TooLarge),
            (WaitError::Invalid("x".into()), C::Invalid),
            (WaitError::Inactive, C::Invalid),
            (WaitError::TargetGone("t".into()), C::NotFound),
            (WaitError::Storage("down".into()), C::Unavailable),
        ] {
            assert_eq!(wait_error(error).code, code);
        }
        // Platform detail never reaches the caller.
        let storage = wait_error(WaitError::Storage("secret dsn".into()));
        assert!(!storage.message.contains("secret"));
        assert_eq!(storage.retry_after_ms, Some(1_000));
        assert!(
            wait_error(WaitError::TargetGone("kid-7".into()))
                .message
                .contains("kid-7")
        );
    }

    /// Decision D1 for waits is control's decision for the commands that
    /// reach direct children only.
    #[test]
    fn authorization_is_decision_d1_for_waits() {
        use super::super::control::{Mutation, decide};
        use runtara_component_host::control_host::ControlErrorCode as E;
        let control_code = |code| match code {
            InstanceWaitErrorCode::Invalid => E::Invalid,
            InstanceWaitErrorCode::Denied => E::Denied,
            InstanceWaitErrorCode::NotChild => E::NotChild,
            other => panic!("D1 never answers {other:?}"),
        };
        for relation in [
            Relation::SelfCall,
            Relation::Child,
            Relation::Ancestor,
            Relation::Other,
        ] {
            assert_eq!(
                authorize(relation).map_err(control_code),
                decide(Mutation::Cancel, relation, true),
                "{relation:?}"
            );
            if let Err(code) = authorize(relation) {
                assert_eq!(refused(code).code, code);
            }
        }
    }

    #[test]
    fn requests_are_normalized_and_checked_before_any_read() {
        let spec = wait_spec(request(vec!["b".into(), "a".into(), "b".into()])).unwrap();
        assert_eq!(spec.targets(), ["a", "b"], "sorted and distinct");
        let at_cap: Vec<String> = (0..=MAX_WAIT_TARGETS)
            .map(|i| format!("kid-{}", i % MAX_WAIT_TARGETS))
            .collect();
        assert!(wait_spec(request(at_cap)).is_ok(), "duplicates count once");
        let over: Vec<String> = (0..=MAX_WAIT_TARGETS).map(|i| format!("kid-{i}")).collect();
        assert_eq!(
            wait_spec(request(over)).unwrap_err().code,
            InstanceWaitErrorCode::TooLarge
        );
        for ids in [vec![" ".to_owned()], vec!["x".repeat(MAX_ID_BYTES + 1)]] {
            assert_eq!(
                wait_spec(request(ids)).unwrap_err().code,
                InstanceWaitErrorCode::Invalid
            );
        }
        let mut late = request(vec!["a".into()]);
        late.deadline_ms = Some(u64::MAX);
        assert_eq!(
            wait_spec(late).unwrap_err().code,
            InstanceWaitErrorCode::Invalid
        );
        let mut any = request(vec!["a".into()]);
        any.mode = InstanceWaitMode::Any;
        any.deadline_ms = Some(1_700_000_000_123);
        let spec = wait_spec(any).unwrap();
        assert_eq!(spec.mode(), store::WaitMode::Any);
        assert_eq!(spec.deadline().map(millis), Some(1_700_000_000_123));
    }

    /// The budget is spent value by value: a value over what is left is
    /// omitted without paying, a kept one pays even when it is not JSON.
    #[test]
    fn inline_values_spend_the_budget_in_order() {
        let mut budget = 10;
        assert_eq!(
            inline_within(&mut budget, Some(b"[1,2]".to_vec()), false),
            (Some(b"[1,2]".to_vec()), false)
        );
        assert_eq!(budget, 5);
        assert_eq!(
            inline_within(&mut budget, Some(b"\"toolong\"".to_vec()), false),
            (None, true)
        );
        assert_eq!(budget, 5, "an omitted value costs nothing");
        assert_eq!(
            inline_within(&mut budget, Some(b"nope".to_vec()), false),
            (None, true)
        );
        assert_eq!(budget, 1, "bytes that are not JSON still pay");
        assert_eq!(inline_within(&mut budget, None, true), (None, true));
        assert_eq!(
            inline_within(&mut budget, Some(b"1".to_vec()), false)
                .0
                .as_deref(),
            Some(b"1".as_slice())
        );
        assert_eq!(budget, 0);
    }

    #[tokio::test]
    async fn an_uninstalled_service_is_unavailable_and_foreign_tenants_denied() {
        let waits = InstanceWaits::with_install_wait(Some("tenant".into()), Duration::ZERO);
        let error = waits
            .register(&caller("tenant"), "w", request(vec!["kid".into()]))
            .await
            .unwrap_err();
        assert_eq!(error.code, InstanceWaitErrorCode::Unavailable);
        assert_eq!(
            waits.poll(&caller("tenant"), "w").await.unwrap_err().code,
            InstanceWaitErrorCode::Unavailable
        );
        for tenant in ["other", ""] {
            assert_eq!(
                waits.poll(&caller(tenant), "w").await.unwrap_err().code,
                InstanceWaitErrorCode::Denied
            );
        }
        // Arguments are checked before the runtime is needed.
        let over: Vec<String> = (0..=MAX_WAIT_TARGETS).map(|i| format!("kid-{i}")).collect();
        assert_eq!(
            waits
                .register(&caller("tenant"), "w", request(over))
                .await
                .unwrap_err()
                .code,
            InstanceWaitErrorCode::TooLarge
        );
        assert_eq!(
            waits.poll(&caller("tenant"), " ").await.unwrap_err().code,
            InstanceWaitErrorCode::Invalid
        );
    }
}
