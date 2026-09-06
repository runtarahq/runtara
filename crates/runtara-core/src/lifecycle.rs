// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure lifecycle decisions. Backends evaluate these against locked state and
//! persist the resulting effects atomically; hosts perform execution actions.

use crate::domain::{EventType, InstanceStatus, SignalType};

/// An explicit field update. Keeping a value is different from clearing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<T> {
    /// Preserve the stored value.
    Keep,
    /// Remove the stored value.
    Clear,
    /// Replace the stored value.
    Set(T),
}

/// Domain reasons for suspending execution. Adapters own their encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspensionReason {
    /// Server shutdown requested a restartable suspension.
    Shutdown,
    /// Execution awaits a timer.
    Sleeping,
    /// Execution awaits a durable custom signal.
    WaitingSignal,
}

/// A requested wake time, resolved by the backend's transaction clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeDeadline {
    /// Make the instance eligible for immediate wake.
    Now,
    /// A host-provided durable timer deadline.
    At(chrono::DateTime<chrono::Utc>),
}

/// The identity and disposition of a stored lifecycle command.
#[derive(Debug, Clone, Copy)]
pub struct Command<'a> {
    /// Opaque command identity.
    pub id: &'a str,
    /// Command kind.
    pub kind: SignalType,
    /// Whether this command has already been applied.
    pub acknowledged: bool,
}

/// The exact command observed by a caller.
#[derive(Debug, Clone, Copy)]
pub struct Receipt<'a> {
    /// Observed command identity.
    pub id: &'a str,
    /// Observed command kind.
    pub kind: SignalType,
}

/// Durable effects of a newly accepted lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// New status, or no status change.
    pub status: Option<InstanceStatus>,
    /// Set finished_at to the transaction timestamp.
    pub finish_now: bool,
    /// Clear output and error from the previous execution outcome.
    pub clear_result: bool,
    /// Suspension reason update.
    pub reason: Change<SuspensionReason>,
    /// Wake deadline update.
    pub wake: Change<WakeDeadline>,
    /// Timeline event to append in the same operation.
    pub event: Option<EventType>,
    /// Acknowledge the locked command in the same operation.
    pub acknowledge: bool,
    /// Report a newly terminal instance after committing.
    pub report_completion: bool,
}

/// Result of evaluating an operation against the current stored state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The operation is stale, mismatched, missing, or invalid for this state.
    Rejected,
    /// The exact receipt was already applied; do not repeat durable effects.
    AlreadyApplied,
    /// Apply these effects atomically.
    Applied(Transition),
}

impl Decision {
    /// Compatibility acknowledgment result exposed by transport adapters.
    pub fn accepted(self) -> bool {
        !matches!(self, Self::Rejected)
    }
}

/// Whether a new command may replace the existing slot. Pending cancellation
/// dominates every later command, including another cancel.
pub fn may_replace_command(stored: Option<Command<'_>>) -> bool {
    !stored.is_some_and(|command| command.kind == SignalType::Cancel && !command.acknowledged)
}

/// Validate a receipt and decide its durable effects. For compatibility a
/// matching cancellation can override completed/failed status, including when
/// the runner discovers an unhandled cancel after execution exits.
pub fn acknowledge(
    status: InstanceStatus,
    stored: Option<Command<'_>>,
    receipt: Receipt<'_>,
) -> Decision {
    let Some(command) = stored else {
        return Decision::Rejected;
    };
    if command.id != receipt.id || command.kind != receipt.kind {
        return Decision::Rejected;
    }
    if command.acknowledged {
        return Decision::AlreadyApplied;
    }
    if status.is_terminal() && command.kind != SignalType::Cancel {
        return Decision::Rejected;
    }
    let mut effects = Transition {
        status: None,
        finish_now: false,
        clear_result: false,
        reason: Change::Keep,
        wake: Change::Keep,
        event: None,
        acknowledge: true,
        report_completion: false,
    };
    match command.kind {
        SignalType::Cancel => {
            effects.status = Some(InstanceStatus::Cancelled);
            effects.finish_now = true;
            effects.wake = Change::Clear;
            effects.report_completion = !status.is_terminal();
        }
        SignalType::Pause | SignalType::Shutdown => {
            effects.status = Some(InstanceStatus::Suspended);
            effects.finish_now = true;
            effects.event = Some(EventType::Suspended);
            if command.kind == SignalType::Shutdown {
                effects.reason = Change::Set(SuspensionReason::Shutdown);
                effects.wake = Change::Set(WakeDeadline::Now);
            } else {
                effects.reason = Change::Clear;
                effects.wake = Change::Clear;
            }
        }
        SignalType::Resume => {}
    }
    Decision::Applied(effects)
}

/// Apply a pending cancel only while parked. Running guests keep their command;
/// terminal instances are untouched. Recovery never replays an accepted receipt.
pub fn cancel_parked(status: InstanceStatus, stored: Option<Command<'_>>) -> Decision {
    let Some(command) = stored else {
        return Decision::Rejected;
    };
    if status != InstanceStatus::Suspended
        || command.kind != SignalType::Cancel
        || command.acknowledged
    {
        return Decision::Rejected;
    }
    acknowledge(
        status,
        Some(command),
        Receipt {
            id: command.id,
            kind: command.kind,
        },
    )
}

/// Kind of durable guest suspension; component-specific wake decoding belongs
/// to the host rather than this policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkReason {
    /// Waiting for a timer.
    Timer,
    /// Waiting for a custom signal, optionally with a timeout.
    Signal,
}

/// A durable suspension requested by an exiting guest.
#[derive(Debug, Clone, Copy)]
pub struct ParkRequest {
    /// Why execution is parking.
    pub reason: ParkReason,
    /// Earliest requested timer or signal timeout.
    pub deadline: Option<chrono::DateTime<chrono::Utc>>,
}

/// Park only a currently running instance. Preserve unrelated termination
/// metadata and emit no additional event, matching the invoke suspension path.
pub fn park(status: InstanceStatus, request: ParkRequest) -> Decision {
    if status != InstanceStatus::Running {
        return Decision::Rejected;
    }
    Decision::Applied(Transition {
        status: Some(InstanceStatus::Suspended),
        finish_now: true,
        clear_result: true,
        reason: Change::Set(match request.reason {
            ParkReason::Timer => SuspensionReason::Sleeping,
            ParkReason::Signal => SuspensionReason::WaitingSignal,
        }),
        wake: request.deadline.map_or(Change::Keep, |deadline| {
            Change::Set(WakeDeadline::At(deadline))
        }),
        event: None,
        acknowledge: false,
        report_completion: false,
    })
}

/// Action performed locally after a successful command acknowledgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionAction {
    /// No local execution change (legacy resume command).
    Continue,
    /// Yield execution until explicitly resumed.
    Pause,
    /// End the current execution, including restartable shutdown.
    Stop,
}

/// Interpret a command for the executing host. Durable effects must be accepted
/// before this action is performed.
pub fn execution_action(kind: SignalType) -> ExecutionAction {
    match kind {
        SignalType::Cancel | SignalType::Shutdown => ExecutionAction::Stop,
        SignalType::Pause => ExecutionAction::Pause,
        SignalType::Resume => ExecutionAction::Continue,
    }
}

/// Whether an in-process sleep should return early to deliver this command.
pub fn interrupts_sleep(kind: SignalType) -> bool {
    execution_action(kind) == ExecutionAction::Stop
}

#[cfg(test)]
mod tests {
    use super::*;
    const STATUSES: [InstanceStatus; 6] = [
        InstanceStatus::Pending,
        InstanceStatus::Running,
        InstanceStatus::Suspended,
        InstanceStatus::Completed,
        InstanceStatus::Failed,
        InstanceStatus::Cancelled,
    ];
    const KINDS: [SignalType; 4] = [
        SignalType::Cancel,
        SignalType::Pause,
        SignalType::Resume,
        SignalType::Shutdown,
    ];
    fn command(kind: SignalType, acknowledged: bool) -> Command<'static> {
        Command {
            id: "current",
            kind,
            acknowledged,
        }
    }
    fn receipt(kind: SignalType) -> Receipt<'static> {
        Receipt {
            id: "current",
            kind,
        }
    }

    #[test]
    fn acknowledgment_state_matrix() {
        for status in STATUSES {
            for kind in KINDS {
                let decision = acknowledge(status, Some(command(kind, false)), receipt(kind));
                let terminal = matches!(
                    status,
                    InstanceStatus::Completed | InstanceStatus::Failed | InstanceStatus::Cancelled
                );
                if terminal && kind != SignalType::Cancel {
                    assert_eq!(decision, Decision::Rejected);
                    continue;
                }
                let Decision::Applied(effects) = decision else {
                    panic!("{status:?} {kind:?}")
                };
                let expected = match kind {
                    SignalType::Cancel => (
                        Some(InstanceStatus::Cancelled),
                        true,
                        Change::Keep,
                        Change::Clear,
                        None,
                        !terminal,
                    ),
                    SignalType::Pause => (
                        Some(InstanceStatus::Suspended),
                        true,
                        Change::Clear,
                        Change::Clear,
                        Some(EventType::Suspended),
                        false,
                    ),
                    SignalType::Shutdown => (
                        Some(InstanceStatus::Suspended),
                        true,
                        Change::Set(SuspensionReason::Shutdown),
                        Change::Set(WakeDeadline::Now),
                        Some(EventType::Suspended),
                        false,
                    ),
                    SignalType::Resume => (None, false, Change::Keep, Change::Keep, None, false),
                };
                assert_eq!(
                    (
                        effects.status,
                        effects.finish_now,
                        effects.reason,
                        effects.wake,
                        effects.event,
                        effects.report_completion
                    ),
                    expected
                );
                assert!(effects.acknowledge);
            }
        }
    }

    #[test]
    fn stale_and_repeated_receipts_never_apply_effects() {
        for status in STATUSES {
            for kind in KINDS {
                assert_eq!(acknowledge(status, None, receipt(kind)), Decision::Rejected);
                assert_eq!(
                    acknowledge(status, Some(command(kind, true)), receipt(kind)),
                    Decision::AlreadyApplied
                );
                assert_eq!(
                    acknowledge(
                        status,
                        Some(command(kind, false)),
                        Receipt { id: "old", kind }
                    ),
                    Decision::Rejected
                );
                for other in KINDS {
                    if other != kind {
                        assert_eq!(
                            acknowledge(status, Some(command(kind, true)), receipt(other)),
                            Decision::Rejected
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn only_pending_cancellation_blocks_replacement() {
        assert!(may_replace_command(None));
        for kind in KINDS {
            assert!(may_replace_command(Some(command(kind, true))));
            assert_eq!(
                may_replace_command(Some(command(kind, false))),
                kind != SignalType::Cancel
            );
        }
    }

    #[test]
    fn parked_cancellation_does_not_consume_commands_for_active_or_terminal_instances() {
        for status in STATUSES {
            for kind in KINDS {
                assert_eq!(
                    matches!(
                        cancel_parked(status, Some(command(kind, false))),
                        Decision::Applied(_)
                    ),
                    status == InstanceStatus::Suspended && kind == SignalType::Cancel
                );
                assert_eq!(
                    cancel_parked(status, Some(command(kind, true))),
                    Decision::Rejected
                );
            }
            assert_eq!(cancel_parked(status, None), Decision::Rejected);
        }
    }

    #[test]
    fn parking_requires_running_and_preserves_unrelated_fields() {
        let deadline = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        for status in STATUSES {
            for reason in [ParkReason::Timer, ParkReason::Signal] {
                for wake in [None, Some(deadline)] {
                    let decision = park(
                        status,
                        ParkRequest {
                            reason,
                            deadline: wake,
                        },
                    );
                    if status != InstanceStatus::Running {
                        assert_eq!(decision, Decision::Rejected);
                        continue;
                    }
                    let Decision::Applied(effects) = decision else {
                        panic!("running must park")
                    };
                    assert_eq!(effects.status, Some(InstanceStatus::Suspended));
                    assert!(effects.finish_now && effects.clear_result);
                    assert!(!effects.acknowledge && !effects.report_completion);
                    assert_eq!(effects.event, None);
                    assert_eq!(
                        effects.reason,
                        Change::Set(if reason == ParkReason::Timer {
                            SuspensionReason::Sleeping
                        } else {
                            SuspensionReason::WaitingSignal
                        })
                    );
                    assert_eq!(
                        effects.wake,
                        wake.map_or(Change::Keep, |d| Change::Set(WakeDeadline::At(d)))
                    );
                }
            }
        }
    }

    #[test]
    fn execution_and_sleep_actions() {
        for (kind, action, interrupts) in [
            (SignalType::Cancel, ExecutionAction::Stop, true),
            (SignalType::Shutdown, ExecutionAction::Stop, true),
            (SignalType::Pause, ExecutionAction::Pause, false),
            (SignalType::Resume, ExecutionAction::Continue, false),
        ] {
            assert_eq!(execution_action(kind), action);
            assert_eq!(interrupts_sleep(kind), interrupts);
        }
    }
}
