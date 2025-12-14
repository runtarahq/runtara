// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Type tests for runtara-sdk.

use runtara_sdk::{CheckpointResult, InstanceStatus, SignalType, SleepResult};

#[test]
fn test_instance_status_from_i32() {
    // Valid statuses
    assert_eq!(InstanceStatus::from(0), InstanceStatus::Unknown);
    assert_eq!(InstanceStatus::from(1), InstanceStatus::Pending);
    assert_eq!(InstanceStatus::from(2), InstanceStatus::Running);
    assert_eq!(InstanceStatus::from(3), InstanceStatus::Suspended);
    assert_eq!(InstanceStatus::from(4), InstanceStatus::Completed);
    assert_eq!(InstanceStatus::from(5), InstanceStatus::Failed);
    assert_eq!(InstanceStatus::from(6), InstanceStatus::Cancelled);

    // Invalid status defaults to Unknown
    assert_eq!(InstanceStatus::from(99), InstanceStatus::Unknown);
    assert_eq!(InstanceStatus::from(-1), InstanceStatus::Unknown);
}

#[test]
fn test_signal_type_from_i32() {
    assert_eq!(SignalType::from(0), SignalType::Cancel);
    assert_eq!(SignalType::from(1), SignalType::Pause);
    assert_eq!(SignalType::from(2), SignalType::Resume);

    // Invalid defaults to Cancel
    assert_eq!(SignalType::from(99), SignalType::Cancel);
}

#[test]
fn test_checkpoint_result_existing_state() {
    // When found = true, existing_state() returns Some
    let found_result = CheckpointResult {
        found: true,
        state: vec![1, 2, 3],
        pending_signal: None,
    };
    assert!(found_result.existing_state().is_some());
    assert_eq!(found_result.existing_state().unwrap(), &[1, 2, 3]);

    // When found = false, existing_state() returns None
    let new_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: None,
    };
    assert!(new_result.existing_state().is_none());
}

#[test]
fn test_checkpoint_result_should_pause() {
    let pause_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(SignalType::Pause),
    };
    assert!(pause_result.should_pause());
    assert!(!pause_result.should_cancel());
    assert!(pause_result.should_exit());

    let no_signal = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: None,
    };
    assert!(!no_signal.should_pause());
}

#[test]
fn test_checkpoint_result_should_cancel() {
    let cancel_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(SignalType::Cancel),
    };
    assert!(cancel_result.should_cancel());
    assert!(!cancel_result.should_pause());
    assert!(cancel_result.should_exit());
}

#[test]
fn test_checkpoint_result_should_exit() {
    // Pause signal - should exit
    let pause_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(SignalType::Pause),
    };
    assert!(pause_result.should_exit());

    // Cancel signal - should exit
    let cancel_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(SignalType::Cancel),
    };
    assert!(cancel_result.should_exit());

    // Resume signal - should NOT exit
    let resume_result = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: Some(SignalType::Resume),
    };
    assert!(!resume_result.should_exit());

    // No signal - should NOT exit
    let no_signal = CheckpointResult {
        found: false,
        state: vec![],
        pending_signal: None,
    };
    assert!(!no_signal.should_exit());
}

#[test]
fn test_sleep_result_deferred() {
    let deferred = SleepResult { deferred: true };
    assert!(deferred.deferred);

    let not_deferred = SleepResult { deferred: false };
    assert!(!not_deferred.deferred);
}

#[test]
fn test_instance_status_debug() {
    let status = InstanceStatus::Running;
    let debug_str = format!("{:?}", status);
    assert!(debug_str.contains("Running"));
}

#[test]
fn test_signal_type_debug() {
    let signal = SignalType::Pause;
    let debug_str = format!("{:?}", signal);
    assert!(debug_str.contains("Pause"));
}

#[test]
fn test_checkpoint_result_debug() {
    let result = CheckpointResult {
        found: true,
        state: vec![1, 2, 3],
        pending_signal: Some(SignalType::Cancel),
    };
    let debug_str = format!("{:?}", result);
    assert!(debug_str.contains("found"));
    assert!(debug_str.contains("true"));
    assert!(debug_str.contains("Cancel"));
}

#[test]
fn test_instance_status_equality() {
    assert_eq!(InstanceStatus::Running, InstanceStatus::Running);
    assert_ne!(InstanceStatus::Running, InstanceStatus::Completed);
}

#[test]
fn test_signal_type_equality() {
    assert_eq!(SignalType::Cancel, SignalType::Cancel);
    assert_ne!(SignalType::Cancel, SignalType::Pause);
}

#[test]
fn test_checkpoint_result_clone() {
    let original = CheckpointResult {
        found: true,
        state: vec![1, 2, 3],
        pending_signal: Some(SignalType::Pause),
    };
    let cloned = original.clone();

    assert_eq!(original.found, cloned.found);
    assert_eq!(original.state, cloned.state);
    assert_eq!(original.pending_signal, cloned.pending_signal);
}

#[test]
fn test_sleep_result_copy() {
    let original = SleepResult { deferred: true };
    let copied = original;
    let another = original; // Should compile because SleepResult implements Copy

    assert_eq!(original.deferred, copied.deferred);
    assert_eq!(original.deferred, another.deferred);
}
