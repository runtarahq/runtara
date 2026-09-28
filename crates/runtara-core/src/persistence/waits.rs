//! Durable instance waits: a run (the waiter) waits for a fixed set of other
//! runs of its tenant (the targets) to finish.
//!
//! A wait is keyed by the waiter and a `wait_id` the host derives from the
//! waiting step's identity (a sha256 hex, like an `op_hash`), so every replay
//! of the step finds the same wait. Its identity is the sorted, distinct
//! target set and the mode ([`WaitSpec::fingerprint`]); registering the same
//! wait id with another identity is a [`WaitError::Conflict`], and the first
//! registration's deadline stands.
//!
//! One rule decides a wait, on the store's clock, in this order
//! ([`resolve`]):
//!
//! 1. no targets: [`WaitResolution::Empty`];
//! 2. the mode holds (`all`: every target finished; `any`: at least one):
//!    [`WaitResolution::Satisfied`];
//! 3. the clock, truncated to milliseconds, is at or past the deadline:
//!    [`WaitResolution::Deadline`], with the targets that did finish;
//! 4. otherwise the wait is pending.
//!
//! A target has finished when its instance row is terminal, or, with no
//! instance row, when a never-launched outcome is published for it: the row
//! wins over an outcome. A target with neither is unknown to the store (a
//! child still in admission, or one whose rows are gone); it counts as not
//! finished, and the caller decides which it is.
//!
//! The first evaluation that resolves a wait persists the resolution and the
//! targets that had finished by then, in finish order. Later reads return
//! that selection, so a replayed `any` can never choose differently.
//!
//! Parking on waits ([`crate::persistence::Persistence::park_instance_on_targets`])
//! re-evaluates them in the park's own transaction and schedules the wake of
//! a wait that already resolved. A target finishing wakes a parked waiter
//! whose wait now holds, and [`InstanceWaits::reconcile_wait_wakes`] is the
//! authoritative repair for any wake the store could not stamp at once.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::domain::InstanceStatus;
use crate::persistence::ExternalOutcomeKind;

/// Most targets one wait may name.
pub const MAX_WAIT_TARGETS: usize = 1000;

/// Longest wait id (a sha256 hex) a store accepts.
pub const MAX_WAIT_ID_BYTES: usize = 128;

/// Longest target instance id a store accepts.
pub const MAX_TARGET_ID_BYTES: usize = 256;

/// [`InstanceWaits::reconcile_wait_wakes`] re-reads every parked wait from
/// the rows on one poll in this many (the first poll included), besides
/// following wake nudges on every poll.
pub const FULL_RECONCILE_EVERY: u64 = 12;

/// When a wait is satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaitMode {
    /// Every target has finished.
    All,
    /// At least one target has finished.
    Any,
}

impl WaitMode {
    /// Storage spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Any => "any",
        }
    }

    /// Parse the storage spelling.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "any" => Some(Self::Any),
            _ => None,
        }
    }
}

/// How a wait resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaitResolution {
    /// The mode held.
    Satisfied,
    /// The deadline passed first.
    Deadline,
    /// The wait has no targets.
    Empty,
}

impl WaitResolution {
    /// Storage spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Deadline => "deadline",
            Self::Empty => "empty",
        }
    }

    /// Parse the storage spelling.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "satisfied" => Some(Self::Satisfied),
            "deadline" => Some(Self::Deadline),
            "empty" => Some(Self::Empty),
            _ => None,
        }
    }
}

/// `at` with its sub-millisecond part dropped: deadlines are stored and
/// compared at millisecond precision.
pub fn truncate_ms(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(at.timestamp_millis()).unwrap_or(at)
}

/// What a wait waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitSpec {
    targets: Vec<String>,
    mode: WaitMode,
    deadline: Option<DateTime<Utc>>,
}

impl WaitSpec {
    /// A wait on `targets` (sorted bytewise and deduplicated here) in `mode`,
    /// until `deadline` (truncated to milliseconds).
    pub fn new(
        targets: impl IntoIterator<Item = String>,
        mode: WaitMode,
        deadline: Option<DateTime<Utc>>,
    ) -> Self {
        let mut targets: Vec<String> = targets.into_iter().collect();
        targets.sort();
        targets.dedup();
        Self {
            targets,
            mode,
            deadline: deadline.map(truncate_ms),
        }
    }

    /// The sorted, distinct targets.
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    /// The mode.
    pub fn mode(&self) -> WaitMode {
        self.mode
    }

    /// The requested deadline, at millisecond precision.
    pub fn deadline(&self) -> Option<DateTime<Utc>> {
        self.deadline
    }

    /// `v1:` plus the hex sha256 of the mode and the sorted, distinct
    /// targets. The deadline is deliberately not part of it: a replay whose
    /// deadline was computed again later is still the same wait.
    pub fn fingerprint(&self) -> String {
        fingerprint(self.mode, &self.targets)
    }

    /// Reject a wait no store may register for `waiter`.
    pub fn validate(&self, waiter: &str, wait_id: &str) -> WaitResult<()> {
        if waiter.trim().is_empty() {
            return Err(WaitError::Invalid("the waiter must be set".into()));
        }
        if wait_id.is_empty() || wait_id.len() > MAX_WAIT_ID_BYTES {
            return Err(WaitError::Invalid(format!(
                "a wait id must be 1-{MAX_WAIT_ID_BYTES} bytes"
            )));
        }
        if self.targets.len() > MAX_WAIT_TARGETS {
            return Err(WaitError::TooLarge);
        }
        for target in &self.targets {
            if target.trim().is_empty() || target.len() > MAX_TARGET_ID_BYTES {
                return Err(WaitError::Invalid(format!(
                    "a target id must be 1-{MAX_TARGET_ID_BYTES} bytes"
                )));
            }
            if target == waiter {
                return Err(WaitError::Invalid("a run cannot wait on itself".into()));
            }
        }
        Ok(())
    }
}

/// The fingerprint of a wait on `targets` (sorted and distinct) in `mode`.
pub fn fingerprint(mode: WaitMode, targets: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(mode.as_str().as_bytes());
    for target in targets {
        hash.update([0x1f]);
        hash.update(target.as_bytes());
    }
    format!("v1:{:x}", hash.finalize())
}

/// What the store knows of one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetState {
    /// The target has an instance row (which wins over any outcome).
    Instance {
        /// Its lifecycle status.
        status: InstanceStatus,
        /// When it finished, once terminal.
        finished_at: Option<DateTime<Utc>>,
    },
    /// No instance row; the child never launched and its outcome is
    /// published.
    Outcome {
        /// `not_started` or `cancelled`.
        outcome: ExternalOutcomeKind,
        /// When the outcome was published (its finish).
        published_at: DateTime<Utc>,
    },
    /// Neither: still in admission, or gone. Not finished.
    Unknown,
}

impl TargetState {
    /// Whether the target has finished.
    pub fn is_finished(&self) -> bool {
        match self {
            Self::Instance { status, .. } => status.is_terminal(),
            Self::Outcome { .. } => true,
            Self::Unknown => false,
        }
    }

    /// When a finished target finished.
    pub fn finished_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Instance {
                status,
                finished_at,
            } if status.is_terminal() => *finished_at,
            Self::Outcome { published_at, .. } => Some(*published_at),
            _ => None,
        }
    }
}

/// One target and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitTarget {
    /// The target's instance id.
    pub instance_id: String,
    /// What the store knows of it.
    pub state: TargetState,
}

/// The rule, pure: how a wait in `mode` over `targets` (their current
/// states) with `deadline` resolves at `now`, or `None` while pending.
pub fn resolve(
    mode: WaitMode,
    targets: &[WaitTarget],
    deadline: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<WaitResolution> {
    if targets.is_empty() {
        return Some(WaitResolution::Empty);
    }
    let finished = targets.iter().filter(|t| t.state.is_finished()).count();
    let holds = match mode {
        WaitMode::All => finished == targets.len(),
        WaitMode::Any => finished > 0,
    };
    if holds {
        return Some(WaitResolution::Satisfied);
    }
    if deadline.is_some_and(|deadline| truncate_ms(now) >= deadline) {
        return Some(WaitResolution::Deadline);
    }
    None
}

/// The finished targets, in finish order (ties broken bytewise by id).
pub fn finished_in_order(targets: &[WaitTarget]) -> Vec<String> {
    let mut finished: Vec<(Option<DateTime<Utc>>, &str)> = targets
        .iter()
        .filter(|t| t.state.is_finished())
        .map(|t| (t.state.finished_at(), t.instance_id.as_str()))
        .collect();
    finished.sort();
    finished.into_iter().map(|(_, id)| id.to_owned()).collect()
}

/// Where a wait is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitState {
    /// Not resolved yet.
    Pending,
    /// Resolved once; the selection is final.
    Resolved {
        /// How it resolved.
        resolution: WaitResolution,
        /// When.
        resolved_at: DateTime<Utc>,
        /// The targets that had finished by then, in finish order.
        finished: Vec<String>,
    },
    /// Closed by its operation; polls answer [`WaitError::Closed`].
    Closed {
        /// When.
        closed_at: DateTime<Utc>,
    },
}

/// One stored wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitRecord {
    /// The waiting run.
    pub waiter_instance_id: String,
    /// The host-derived id of the waiting step.
    pub wait_id: String,
    /// The waiter's tenant.
    pub tenant_id: String,
    /// The mode.
    pub mode: WaitMode,
    /// The sorted, distinct targets.
    pub targets: Vec<String>,
    /// [`WaitSpec::fingerprint`] of the first registration.
    pub fingerprint: String,
    /// The first registration's deadline, at millisecond precision.
    pub deadline: Option<DateTime<Utc>>,
    /// When it was registered.
    pub created_at: DateTime<Utc>,
    /// Its state.
    pub state: WaitState,
}

impl WaitRecord {
    /// The resolution to persist when this pending wait resolves at `now`
    /// over `states` (every target, in [`Self::targets`] order): the rule's
    /// answer and the finished selection. `None` while pending, and for a
    /// wait that is not pending.
    pub fn evaluate(
        &self,
        states: &[WaitTarget],
        now: DateTime<Utc>,
    ) -> Option<(WaitResolution, Vec<String>)> {
        if self.state != WaitState::Pending {
            return None;
        }
        let resolution = resolve(self.mode, states, self.deadline, now)?;
        Some((resolution, finished_in_order(states)))
    }
}

/// A read of a wait: the record plus its targets split into finished and
/// remaining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitView {
    /// The stored wait (pending or resolved; a closed wait is an error).
    pub record: WaitRecord,
    /// Finished targets in finish order: the persisted selection once
    /// resolved, else every target finished now.
    pub finished: Vec<WaitTarget>,
    /// The other targets in target order, with their current state
    /// ([`TargetState::Unknown`] for one the store has no rows for).
    pub remaining: Vec<WaitTarget>,
}

impl WaitView {
    /// Split `states` (every target, in `record.targets` order) by the
    /// record. A target of a resolved selection that the store no longer
    /// knows is [`WaitError::TargetGone`]: its result is never silently
    /// dropped.
    pub fn assemble(record: WaitRecord, states: Vec<WaitTarget>) -> WaitResult<Self> {
        let (finished, remaining) = match &record.state {
            WaitState::Closed { .. } => return Err(WaitError::Closed),
            WaitState::Pending => {
                let order = finished_in_order(&states);
                let mut finished = Vec::with_capacity(order.len());
                let mut remaining = Vec::new();
                let mut by_id: std::collections::HashMap<String, WaitTarget> = states
                    .into_iter()
                    .map(|t| (t.instance_id.clone(), t))
                    .collect();
                for id in &order {
                    finished.extend(by_id.remove(id));
                }
                for id in &record.targets {
                    remaining.extend(by_id.remove(id));
                }
                (finished, remaining)
            }
            WaitState::Resolved {
                finished: selection,
                ..
            } => {
                let mut by_id: std::collections::HashMap<String, WaitTarget> = states
                    .into_iter()
                    .map(|t| (t.instance_id.clone(), t))
                    .collect();
                let mut finished = Vec::with_capacity(selection.len());
                for id in selection {
                    match by_id.remove(id) {
                        Some(target) if target.state != TargetState::Unknown => {
                            finished.push(target)
                        }
                        _ => return Err(WaitError::TargetGone(id.clone())),
                    }
                }
                let remaining = record
                    .targets
                    .iter()
                    .filter_map(|id| by_id.remove(id))
                    .collect();
                (finished, remaining)
            }
        };
        Ok(Self {
            record,
            finished,
            remaining,
        })
    }

    /// The resolution, once resolved.
    pub fn resolution(&self) -> Option<WaitResolution> {
        match &self.record.state {
            WaitState::Resolved { resolution, .. } => Some(*resolution),
            _ => None,
        }
    }
}

/// Why a wait operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WaitError {
    /// No such waiter in the tenant, or no such wait.
    #[error("no such wait")]
    NotFound,
    /// The wait was closed by its operation.
    #[error("the wait is closed")]
    Closed,
    /// The operation already registered a wait on other targets or another
    /// mode.
    #[error("the operation already waits on other targets or in another mode")]
    Conflict,
    /// The waiter has finished; it cannot register waits.
    #[error("the waiting run has finished")]
    Inactive,
    /// More than [`MAX_WAIT_TARGETS`] targets.
    #[error("a wait names at most {MAX_WAIT_TARGETS} targets")]
    TooLarge,
    /// Malformed arguments.
    #[error("invalid wait: {0}")]
    Invalid(String),
    /// A finished target of a resolved wait has no rows any more.
    #[error("target {0} of the wait is no longer retained")]
    TargetGone(String),
    /// The store failed.
    #[error("wait storage: {0}")]
    Storage(String),
}

/// Result of a wait operation.
pub type WaitResult<T> = Result<T, WaitError>;

/// Durable instance waits. Every operation checks that the waiter is a run of
/// `tenant`; a foreign or unknown waiter is [`WaitError::NotFound`].
#[async_trait]
pub trait InstanceWaits: Send + Sync {
    /// Register the wait of `waiter`'s operation `wait_id` on `spec`, or read
    /// back the one it already registered, and evaluate it now.
    ///
    /// A registration with another fingerprint is [`WaitError::Conflict`];
    /// the first deadline stands. A closed wait is replaced by a fresh
    /// registration (a retried operation waits again). A finished waiter is
    /// [`WaitError::Inactive`].
    async fn register_or_evaluate(
        &self,
        tenant: &str,
        waiter: &str,
        wait_id: &str,
        spec: &WaitSpec,
    ) -> WaitResult<WaitView>;

    /// Evaluate a registered wait now, persisting its resolution the first
    /// time it resolves. [`WaitError::NotFound`] for no such wait,
    /// [`WaitError::Closed`] for a closed one.
    async fn poll_wait(&self, tenant: &str, waiter: &str, wait_id: &str) -> WaitResult<WaitView>;

    /// Close a pending or resolved wait. Returns whether this call closed it;
    /// an unknown or already closed wait is `false`.
    async fn close_wait(&self, tenant: &str, waiter: &str, wait_id: &str) -> WaitResult<bool>;

    /// Delete a wait that is no longer pending (resolved or closed), once its
    /// result is checkpointed. Returns whether it was deleted; a pending or
    /// unknown wait is `false`.
    async fn delete_resolved_wait(
        &self,
        tenant: &str,
        waiter: &str,
        wait_id: &str,
    ) -> WaitResult<bool>;

    /// Schedule the wakes of parked waiters whose waits resolved without a
    /// wake being stamped. Every call follows the nudges finishing targets
    /// left (up to `limit` waiters); one call in
    /// [`FULL_RECONCILE_EVERY`] also re-reads up to `limit` parked waits from
    /// the rows, least recently reconciled first. Returns how many waiters it
    /// woke. A waiter is woken once per park, never while paused.
    async fn reconcile_wait_wakes(&self, limit: u32) -> WaitResult<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str, state: TargetState) -> WaitTarget {
        WaitTarget {
            instance_id: id.into(),
            state,
        }
    }

    fn at(ms: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(ms).unwrap()
    }

    fn done(ms: i64) -> TargetState {
        TargetState::Instance {
            status: InstanceStatus::Completed,
            finished_at: Some(at(ms)),
        }
    }

    fn running() -> TargetState {
        TargetState::Instance {
            status: InstanceStatus::Running,
            finished_at: None,
        }
    }

    #[test]
    fn the_spec_is_sorted_distinct_and_its_fingerprint_ignores_the_deadline() {
        let a = WaitSpec::new(
            ["b".to_string(), "a".into(), "b".into()],
            WaitMode::All,
            Some(at(1_000) + chrono::Duration::microseconds(700)),
        );
        assert_eq!(a.targets(), ["a", "b"]);
        assert_eq!(a.deadline(), Some(at(1_000)), "ms-truncated");
        let b = WaitSpec::new(["a".to_string(), "b".into()], WaitMode::All, None);
        assert_eq!(a.fingerprint(), b.fingerprint());
        let any = WaitSpec::new(["a".to_string(), "b".into()], WaitMode::Any, None);
        assert_ne!(a.fingerprint(), any.fingerprint());
        let other = WaitSpec::new(["a".to_string(), "c".into()], WaitMode::All, None);
        assert_ne!(a.fingerprint(), other.fingerprint());
        // Concatenation cannot alias two target sets.
        let joined = WaitSpec::new(["ab".to_string()], WaitMode::All, None);
        let split = WaitSpec::new(["a".to_string(), "b".into()], WaitMode::All, None);
        assert_ne!(joined.fingerprint(), split.fingerprint());
    }

    #[test]
    fn validation_bounds_the_targets() {
        let many = WaitSpec::new(
            (0..=MAX_WAIT_TARGETS).map(|i| format!("t{i}")),
            WaitMode::All,
            None,
        );
        assert_eq!(many.validate("w", "op"), Err(WaitError::TooLarge));
        let at_cap = WaitSpec::new(
            (0..MAX_WAIT_TARGETS).map(|i| format!("t{i}")),
            WaitMode::All,
            None,
        );
        assert!(at_cap.validate("w", "op").is_ok());
        let own = WaitSpec::new(["w".to_string()], WaitMode::Any, None);
        assert!(matches!(
            own.validate("w", "op"),
            Err(WaitError::Invalid(_))
        ));
        let blank = WaitSpec::new([" ".to_string()], WaitMode::Any, None);
        assert!(matches!(
            blank.validate("w", "op"),
            Err(WaitError::Invalid(_))
        ));
        let ok = WaitSpec::new(["a".to_string()], WaitMode::Any, None);
        assert!(matches!(ok.validate("w", ""), Err(WaitError::Invalid(_))));
        assert!(matches!(
            ok.validate("w", &"x".repeat(MAX_WAIT_ID_BYTES + 1)),
            Err(WaitError::Invalid(_))
        ));
    }

    #[test]
    fn one_rule_empty_then_condition_then_deadline() {
        let deadline = Some(at(10_000));
        assert_eq!(
            resolve(WaitMode::All, &[], deadline, at(0)),
            Some(WaitResolution::Empty)
        );
        let partial = [target("a", done(5)), target("b", running())];
        assert_eq!(resolve(WaitMode::All, &partial, deadline, at(9_999)), None);
        assert_eq!(
            resolve(WaitMode::Any, &partial, deadline, at(0)),
            Some(WaitResolution::Satisfied)
        );
        // At the deadline exactly (>=), and sub-millisecond early counts as
        // the same millisecond.
        assert_eq!(
            resolve(WaitMode::All, &partial, deadline, at(10_000)),
            Some(WaitResolution::Deadline)
        );
        assert_eq!(
            resolve(
                WaitMode::All,
                &partial,
                deadline,
                at(10_000) + chrono::Duration::microseconds(999)
            ),
            Some(WaitResolution::Deadline)
        );
        assert_eq!(
            resolve(
                WaitMode::All,
                &partial,
                deadline,
                at(9_999) + chrono::Duration::microseconds(999)
            ),
            None
        );
        // The condition wins over a passed deadline.
        let all = [target("a", done(5)), target("b", done(3))];
        assert_eq!(
            resolve(WaitMode::All, &all, deadline, at(20_000)),
            Some(WaitResolution::Satisfied)
        );
        // Unknown targets are not finished; outcomes are.
        let unknown = [target("a", TargetState::Unknown)];
        assert_eq!(resolve(WaitMode::Any, &unknown, None, at(0)), None);
        let outcome = [target(
            "a",
            TargetState::Outcome {
                outcome: ExternalOutcomeKind::NotStarted,
                published_at: at(1),
            },
        )];
        assert_eq!(
            resolve(WaitMode::All, &outcome, None, at(0)),
            Some(WaitResolution::Satisfied)
        );
        assert_eq!(finished_in_order(&all), ["b", "a"], "finish order");
    }

    fn record(state: WaitState) -> WaitRecord {
        WaitRecord {
            waiter_instance_id: "w".into(),
            wait_id: "op".into(),
            tenant_id: "t".into(),
            mode: WaitMode::Any,
            targets: vec!["a".into(), "b".into(), "c".into()],
            fingerprint: String::new(),
            deadline: None,
            created_at: at(0),
            state,
        }
    }

    #[test]
    fn a_resolved_selection_is_final_and_never_silently_loses_a_target() {
        let resolved = record(WaitState::Resolved {
            resolution: WaitResolution::Satisfied,
            resolved_at: at(10),
            finished: vec!["b".into()],
        });
        // `a` finished later: it stays remaining, the selection does not grow.
        let view = WaitView::assemble(
            resolved.clone(),
            vec![
                target("a", done(20)),
                target("b", done(5)),
                target("c", running()),
            ],
        )
        .unwrap();
        assert_eq!(view.resolution(), Some(WaitResolution::Satisfied));
        assert_eq!(view.finished.len(), 1);
        assert_eq!(view.finished[0].instance_id, "b");
        assert_eq!(
            view.remaining
                .iter()
                .map(|t| t.instance_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert!(resolved.evaluate(&view.finished, at(99)).is_none());
        // A selected target whose rows are gone is an explicit error.
        assert_eq!(
            WaitView::assemble(
                resolved,
                vec![
                    target("a", done(20)),
                    target("b", TargetState::Unknown),
                    target("c", running()),
                ],
            ),
            Err(WaitError::TargetGone("b".into()))
        );
        assert_eq!(
            WaitView::assemble(record(WaitState::Closed { closed_at: at(1) }), vec![]),
            Err(WaitError::Closed)
        );
    }
}
