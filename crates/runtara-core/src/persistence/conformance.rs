// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conformance harness for the persistence backend.
//!
//! Runs a scripted sequence of [`Persistence`] operations and asserts
//! invariants on the observable state between steps. It covers the sleep lifecycle
//! (`set_instance_sleep`, `get_sleeping_instances_due`,
//! `claim_sleeping_instance`, `claim_sleeping_instances_due`,
//! `clear_instance_sleep`) and retention via
//! `get_terminal_instances_older_than` / `delete_instances_batch`.

use crate::domain::InstanceStatus as CoreInstanceStatus;
use crate::domain::SignalType as CoreSignalType;

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::persistence::{
    CompleteInstanceParams, EventRecord, EventVocabulary, EventVocabularySpec, ListEventsFilter,
    ListPairedRecordsFilter, PairedRecordStatus, Persistence,
};

/// Run the full conformance sequence against `backend`.
///
/// Intentionally linear, with no test-specific branches: each step documents
/// the invariant it checks so a failure points at a specific behaviour.
pub async fn run_conformance_sequence<P: Persistence>(backend: &P) {
    let instance_id = Uuid::new_v4().to_string();
    let tenant_id = "conformance-tenant";

    // --- register + get -----------------------------------------------------
    backend
        .register_instance(&instance_id, tenant_id)
        .await
        .expect("register_instance failed");

    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed")
        .expect("instance should exist immediately after register");
    assert_eq!(record.instance_id, instance_id);
    assert_eq!(record.tenant_id, tenant_id);
    assert_eq!(record.status, CoreInstanceStatus::Pending);

    // --- try_register is a claim, not a second insert -----------------------
    // The id is an idempotency key for an at-least-once trigger stream, so a
    // replay has to report "already taken" rather than erroring or clobbering
    // the row that is already mid-launch.
    let claimed_again = backend
        .try_register_instance(&instance_id, tenant_id, Some(b"{\"stolen\":true}"))
        .await
        .expect("try_register_instance on an existing id should not error");
    assert!(
        !claimed_again,
        "try_register_instance must report false for an id that already exists"
    );
    let unchanged = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed")
        .expect("instance should still exist after a losing claim");
    assert_eq!(
        unchanged.tenant_id, tenant_id,
        "a losing claim must not overwrite the existing row"
    );

    // A losing claim must not smuggle its own input onto the existing row.
    assert!(
        backend
            .get_instance(&instance_id)
            .await
            .expect("get_instance failed")
            .expect("instance should still exist")
            .input
            .is_none(),
        "a losing claim must not write its input over the existing row"
    );

    // A winning claim persists the input in the same operation, so no separate
    // store_instance_input is needed on the launch path.
    let fresh_id = Uuid::new_v4().to_string();
    let fresh_input = b"{\"data\":{\"claimed\":true}}".to_vec();
    assert!(
        backend
            .try_register_instance(&fresh_id, tenant_id, Some(&fresh_input))
            .await
            .expect("try_register_instance on a fresh id failed"),
        "try_register_instance must report true when it creates the row"
    );
    let created = backend
        .get_instance(&fresh_id)
        .await
        .expect("get_instance failed")
        .expect("a winning claim must actually insert the row");
    assert_eq!(
        created.input.as_deref(),
        Some(fresh_input.as_slice()),
        "the claim must persist the input it was given"
    );
    assert_eq!(created.status, CoreInstanceStatus::Pending);

    // And a claim with no input leaves the input absent rather than erroring.
    let no_input_id = Uuid::new_v4().to_string();
    assert!(
        backend
            .try_register_instance(&no_input_id, tenant_id, None)
            .await
            .expect("try_register_instance without input failed")
    );
    assert!(
        backend
            .get_instance(&no_input_id)
            .await
            .expect("get_instance failed")
            .expect("row should exist")
            .input
            .is_none()
    );

    // --- get_instance_meta drops the input and nothing else -----------------
    // The projection exists to keep status checks off the launch payload, so
    // the contract is narrow: `input` comes back None, every other field comes
    // back exactly as the full read gives it. A field quietly falling to its
    // Default here would be a silent data bug at the call sites that swapped.
    let payload = b"{\"data\":{\"conformance\":true}}".to_vec();
    backend
        .store_instance_input(&instance_id, &payload)
        .await
        .expect("store_instance_input failed");

    let full = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed")
        .expect("instance should exist");
    assert_eq!(
        full.input.as_deref(),
        Some(payload.as_slice()),
        "the full read must still return the stored input"
    );

    let meta = backend
        .get_instance_meta(&instance_id)
        .await
        .expect("get_instance_meta failed")
        .expect("instance should exist");
    assert!(
        meta.input.is_none(),
        "get_instance_meta must not return the input blob"
    );
    assert_eq!(meta.instance_id, full.instance_id);
    assert_eq!(meta.tenant_id, full.tenant_id);
    assert_eq!(meta.definition_version, full.definition_version);
    assert_eq!(meta.status, full.status);
    assert_eq!(meta.termination_reason, full.termination_reason);
    assert_eq!(meta.checkpoint_id, full.checkpoint_id);
    assert_eq!(meta.attempt, full.attempt);
    assert_eq!(meta.max_attempts, full.max_attempts);
    assert_eq!(meta.created_at, full.created_at);
    assert_eq!(meta.started_at, full.started_at);
    assert_eq!(meta.finished_at, full.finished_at);
    assert_eq!(meta.output, full.output);
    assert_eq!(meta.error, full.error);
    assert_eq!(meta.sleep_until, full.sleep_until);
    assert_eq!(meta.recovery_attempts, full.recovery_attempts);
    assert_eq!(meta.recovery_marker, full.recovery_marker);

    assert!(
        backend
            .get_instance_meta("no-such-instance-for-conformance")
            .await
            .expect("get_instance_meta on a missing id should not error")
            .is_none(),
        "get_instance_meta must report a missing instance as None"
    );

    // --- claiming a sleeper leases it, it does not clear it -----------------
    // A claim that cleared `sleep_until` would leave the row `suspended` with
    // no deadline, which is exactly what a signal waiter looks like. Nothing
    // could then tell them apart, so a process that died between claiming and
    // launching would strand its whole batch permanently. Leasing keeps a
    // deadline on the row so it simply becomes due again.
    let sleeper = Uuid::new_v4().to_string();
    backend
        .register_instance(&sleeper, tenant_id)
        .await
        .expect("register sleeper failed");
    backend
        .update_instance_status(&sleeper, CoreInstanceStatus::Suspended, None)
        .await
        .expect("suspend sleeper failed");
    backend
        .set_instance_sleep(&sleeper, Utc::now() - chrono::Duration::seconds(30))
        .await
        .expect("set_instance_sleep failed");

    // The lib tests share one store, so a rival test polling the same due
    // set may take this row first. Claim in a bounded loop and accept either
    // outcome: what must hold is that whoever claimed it left a deadline
    // behind. `SKIP LOCKED` also means one round need not see every row.
    let lease_until = Utc::now() + chrono::Duration::seconds(120);
    let mut claimed_by_us = false;
    for _ in 0..10 {
        let batch = backend
            .claim_sleeping_instances_due(200, lease_until)
            .await
            .expect("claim_sleeping_instances_due failed");
        if batch.iter().any(|r| r.instance_id == sleeper) {
            claimed_by_us = true;
            break;
        }
        if batch.is_empty() {
            break;
        }
    }

    let leased = backend
        .get_instance(&sleeper)
        .await
        .expect("get_instance failed")
        .expect("sleeper should exist");
    assert!(
        leased.sleep_until.is_some(),
        "a claim must leave a recovery deadline, not clear it"
    );

    // Held: it is not offered again while the lease is live.
    if claimed_by_us {
        assert!(
            !backend
                .get_sleeping_instances_due(200)
                .await
                .expect("get_sleeping_instances_due failed")
                .iter()
                .any(|r| r.instance_id == sleeper),
            "a leased claim must not be handed out again while the lease holds"
        );
    }

    // Expired: the interrupted-wake recovery path. Nothing else runs here, so
    // this stands in for the process that claimed it never coming back.
    backend
        .set_instance_sleep(&sleeper, Utc::now() - chrono::Duration::seconds(1))
        .await
        .expect("expire lease failed");
    let mut reclaimed = false;
    for _ in 0..10 {
        let batch = backend
            .claim_sleeping_instances_due(200, Utc::now() + chrono::Duration::seconds(120))
            .await
            .expect("reclaim failed");
        if batch.iter().any(|r| r.instance_id == sleeper) {
            reclaimed = true;
            break;
        }
        if batch.is_empty() {
            break;
        }
    }
    assert!(
        reclaimed || {
            // A rival may have taken it; it still must not be left deadline-less.
            backend
                .get_instance(&sleeper)
                .await
                .expect("get_instance failed")
                .expect("sleeper should exist")
                .sleep_until
                .is_some()
        },
        "once the lease expires the sleeper must become claimable again"
    );

    // --- mark_instance_running: relaunch promotion --------------------------
    // Wake and resume promote from `suspended`, which `mark_instance_started`
    // refuses on purpose, and the original `started_at` has to survive so a run
    // that suspends and wakes still reports when it first began.
    let first_started = Utc::now() - chrono::Duration::seconds(120);
    backend
        .update_instance_status(
            &instance_id,
            CoreInstanceStatus::Running,
            Some(first_started),
        )
        .await
        .expect("seed running failed");
    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Suspended, None)
        .await
        .expect("suspend failed");
    let before = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed")
        .expect("instance should exist");
    assert_eq!(before.status, CoreInstanceStatus::Suspended);

    backend
        .mark_instance_running(&instance_id, Utc::now())
        .await
        .expect("mark_instance_running failed");
    let promoted = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed")
        .expect("instance should exist");
    assert_eq!(
        promoted.status,
        CoreInstanceStatus::Running,
        "mark_instance_running must promote a suspended instance"
    );
    assert_eq!(
        promoted.started_at, before.started_at,
        "mark_instance_running must keep the original started_at"
    );
    assert!(promoted.finished_at.is_none());

    // --- update status → running -------------------------------------------
    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
        .await
        .expect("update_instance_status running failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after update failed")
        .expect("instance must still exist");
    assert_eq!(record.status, CoreInstanceStatus::Running);
    assert!(record.started_at.is_some());

    // --- checkpoints --------------------------------------------------------
    let checkpoint_id = "ckpt-1";
    let state = b"opaque-state".to_vec();
    backend
        .save_checkpoint(&instance_id, checkpoint_id, &state)
        .await
        .expect("save_checkpoint failed");
    let loaded = backend
        .load_checkpoint(&instance_id, checkpoint_id)
        .await
        .expect("load_checkpoint failed")
        .expect("checkpoint should load immediately after save");
    assert_eq!(loaded.checkpoint_id, checkpoint_id);
    assert_eq!(loaded.state, state);

    let checkpoints = backend
        .list_checkpoints(&instance_id, None, 50, 0, None, None)
        .await
        .expect("list_checkpoints failed");
    assert!(
        checkpoints.iter().any(|c| c.checkpoint_id == checkpoint_id),
        "saved checkpoint must appear in list_checkpoints"
    );

    let count = backend
        .count_checkpoints(&instance_id, None, None, None)
        .await
        .expect("count_checkpoints failed");
    assert!(count >= 1);

    // Filter: positive match by checkpoint_id.
    let filtered = backend
        .list_checkpoints(&instance_id, Some(checkpoint_id), 50, 0, None, None)
        .await
        .expect("list_checkpoints with filter failed");
    assert!(filtered.iter().all(|c| c.checkpoint_id == checkpoint_id));
    // Filter: negative match by checkpoint_id returns empty.
    let empty = backend
        .list_checkpoints(&instance_id, Some("ckpt-does-not-exist"), 50, 0, None, None)
        .await
        .expect("list_checkpoints with non-matching filter failed");
    assert!(empty.is_empty());
    let filtered_count = backend
        .count_checkpoints(&instance_id, Some(checkpoint_id), None, None)
        .await
        .expect("count_checkpoints with filter failed");
    assert!(filtered_count >= 1);

    // Re-save the same key. The engine replays from the start and reads
    // checkpoints as a result cache, so this is the ordinary path on a
    // resume — every backend must accept it, refresh the state, and leave
    // exactly one row behind. A backend that inserts without an upsert
    // fails here on the save itself.
    let refreshed_state = b"opaque-state-v2".to_vec();
    backend
        .save_checkpoint(&instance_id, checkpoint_id, &refreshed_state)
        .await
        .expect("re-saving an existing checkpoint key must succeed");
    let reloaded = backend
        .load_checkpoint(&instance_id, checkpoint_id)
        .await
        .expect("load_checkpoint after re-save failed")
        .expect("checkpoint must still exist after re-save");
    assert_eq!(
        reloaded.state, refreshed_state,
        "re-save must refresh the stored state, not keep the original"
    );
    let count_after_resave = backend
        .count_checkpoints(&instance_id, Some(checkpoint_id), None, None)
        .await
        .expect("count_checkpoints after re-save failed");
    assert_eq!(
        count_after_resave, 1,
        "re-save must replace the row, not add a second one"
    );
    let total_after_resave = backend
        .count_checkpoints(&instance_id, None, None, None)
        .await
        .expect("unfiltered count_checkpoints after re-save failed");
    assert_eq!(
        total_after_resave, 1,
        "only one checkpoint has been saved for this instance"
    );

    // --- update instance checkpoint pointer --------------------------------
    backend
        .update_instance_checkpoint(&instance_id, checkpoint_id)
        .await
        .expect("update_instance_checkpoint failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after checkpoint update failed")
        .expect("instance must still exist");
    assert_eq!(record.checkpoint_id.as_deref(), Some(checkpoint_id));

    // --- events -------------------------------------------------------------
    // Backdated deliberately. `created_at` is the emitter's observation time
    // and must survive the round trip untouched; a backend that defaults the
    // field to its own write time stamps this event five minutes late. An
    // event created at `Utc::now()` would read back the same under either
    // behaviour, so it could not tell them apart.
    let emitted_at = Utc::now() - Duration::minutes(5);
    let event = EventRecord {
        id: None,
        instance_id: instance_id.clone(),
        event_type: crate::domain::EventType::Custom,
        checkpoint_id: Some(checkpoint_id.to_string()),
        payload: Some(br#"{"note":"hello"}"#.to_vec()),
        created_at: emitted_at,
        subtype: Some("conformance-test".to_string()),
    };
    backend
        .insert_event(&event)
        .await
        .expect("insert_event failed");

    let filter = ListEventsFilter::default();
    let events = backend
        .list_events(&instance_id, &filter, 50, 0)
        .await
        .expect("list_events failed");
    assert!(
        !events.is_empty(),
        "list_events must return the inserted event"
    );

    let stored = events
        .iter()
        .find(|e| e.subtype.as_deref() == Some("conformance-test"))
        .expect("the inserted event must come back from list_events");
    let drift_ms = (stored.created_at - emitted_at).num_milliseconds().abs();
    assert!(
        drift_ms < 1_000,
        "insert_event must persist the caller's created_at: emitted {emitted_at}, \
         stored {}, drift {drift_ms}ms — a backend defaulting the field to its \
         own write time drifts by the full backdate",
        stored.created_at
    );

    let event_count = backend
        .count_events(&instance_id, &filter)
        .await
        .expect("count_events failed");
    assert!(event_count >= 1);

    // --- signals ------------------------------------------------------------
    let signal_payload = br#"{"reason":"parity"}"#.to_vec();
    backend
        .insert_signal(&instance_id, CoreSignalType::Cancel, &signal_payload)
        .await
        .expect("insert_signal failed");
    let pending = backend
        .get_pending_signal(&instance_id)
        .await
        .expect("get_pending_signal failed")
        .expect("signal should be pending after insert");
    assert_eq!(pending.signal_type, CoreSignalType::Cancel);
    backend
        .acknowledge_signal(&instance_id)
        .await
        .expect("acknowledge_signal failed");
    // The ack consumes the signal: a second read must come back empty.
    // Re-reading is the whole point — a guest acknowledges on read, and a
    // redelivered cancel would re-suspend a relaunched instance on a signal it
    // already handled.
    assert!(
        backend
            .get_pending_signal(&instance_id)
            .await
            .expect("get_pending_signal after ack failed")
            .is_none(),
        "an acknowledged signal must not be delivered again"
    );
    // A genuinely new signal for the same instance is still delivered: the
    // insert resets the acknowledgement.
    backend
        .insert_signal(&instance_id, CoreSignalType::Shutdown, b"drain")
        .await
        .expect("insert_signal after ack failed");
    let reinserted = backend
        .get_pending_signal(&instance_id)
        .await
        .expect("get_pending_signal after re-insert failed")
        .expect("a freshly inserted signal must be pending again");
    assert_eq!(reinserted.signal_type, CoreSignalType::Shutdown);
    assert!(reinserted.acknowledged_at.is_none());

    // --- custom checkpoint signals -----------------------------------------
    let custom_payload = br#"{"wait-key":"payment"}"#.to_vec();
    backend
        .insert_custom_signal(&instance_id, checkpoint_id, &custom_payload)
        .await
        .expect("insert_custom_signal failed");
    let taken = backend
        .take_pending_custom_signal(&instance_id, checkpoint_id)
        .await
        .expect("take_pending_custom_signal failed")
        .expect("custom signal should be readable");
    assert_eq!(taken.checkpoint_id, checkpoint_id);
    // Reads are non-destructive: a replayed WaitForSignal re-reads the same
    // signal after a drain/resume, so a second read returns the row again
    // rather than None. Instance deletion reclaims the signal.
    let taken_again = backend
        .take_pending_custom_signal(&instance_id, checkpoint_id)
        .await
        .expect("take_pending_custom_signal second call failed")
        .expect("custom signal must remain re-readable (non-destructive)");
    assert_eq!(taken_again.checkpoint_id, checkpoint_id);
    assert_eq!(taken_again.payload, taken.payload);

    // --- paired records -----------------------------------------------------
    // This harness emits none of this vocabulary's start events, so the paired
    // query must come back empty rather than surfacing this instance's other
    // events. Content-level pairing coverage lives in each backend's tests.
    let vocabulary = EventVocabulary::new(EventVocabularySpec {
        start_subtype: "conformance_start",
        end_subtype: "conformance_end",
        correlation_key: "unit_id",
        kind_key: "unit_kind",
        label_key: "unit_label",
        inputs_key: "given",
        outputs_key: "produced",
        error_key: "failure",
        error_flag_key: "_failed",
        launched_at_key: "began_ms",
        settled_at_key: "ended_ms",
    })
    .expect("valid vocabulary");
    let paired_filter = ListPairedRecordsFilter::default();
    let paired_records = backend
        .list_paired_records(&instance_id, &vocabulary, &paired_filter, 50, 0)
        .await
        .expect("list_paired_records failed");
    assert!(paired_records.is_empty());
    assert_eq!(
        backend
            .count_paired_records(&instance_id, &vocabulary, &paired_filter)
            .await
            .expect("count_paired_records failed"),
        0,
        "the count must agree with the (empty) listing"
    );
    // Bind the variant so the match remains type-checked when we add status-filtered cases.
    let _ = PairedRecordStatus::Running;

    // --- sleep cycle --------------------------------------------------------
    // Verifies both the "not due yet" (running) and "due now" (suspended +
    // past sleep_until) cases: `op_get_sleeping_instances_due` reports an
    // instance only once its status is 'suspended' and its `sleep_until` has
    // gone by, so a running instance parked in the past stays invisible.
    let wake_at = Utc::now() - Duration::seconds(30);
    backend
        .set_instance_sleep(&instance_id, wake_at)
        .await
        .expect("set_instance_sleep failed");
    let due = backend
        .get_sleeping_instances_due(50)
        .await
        .expect("get_sleeping_instances_due failed");
    assert!(
        due.iter().all(|r| r.instance_id != instance_id),
        "instance in 'running' must not appear as due to wake"
    );
    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Suspended, None)
        .await
        .expect("update_instance_status suspended failed");
    let due = backend
        .get_sleeping_instances_due(50)
        .await
        .expect("get_sleeping_instances_due failed (after suspend)");
    assert!(
        due.iter().any(|r| r.instance_id == instance_id),
        "suspended instance with past sleep_until must be due to wake"
    );

    // --- atomic claim (double-launch prevention) ----------------------------
    // The instance is suspended with a past sleep_until (due). The first claim
    // must win and clear sleep_until; a second claim must lose — this is what
    // stops two wakers (or two Environments sharing this store) from
    // launching the same instance twice.
    let first_claim = backend
        .claim_sleeping_instance(&instance_id)
        .await
        .expect("claim_sleeping_instance (first) failed");
    assert!(first_claim, "first claim of a due instance must win");
    let due_after_claim = backend
        .get_sleeping_instances_due(50)
        .await
        .expect("get_sleeping_instances_due failed (after claim)");
    assert!(
        due_after_claim.iter().all(|r| r.instance_id != instance_id),
        "a claimed instance must no longer be due to wake"
    );
    let second_claim = backend
        .claim_sleeping_instance(&instance_id)
        .await
        .expect("claim_sleeping_instance (second) failed");
    assert!(
        !second_claim,
        "second claim of an already-claimed instance must lose"
    );

    backend
        .clear_instance_sleep(&instance_id)
        .await
        .expect("clear_instance_sleep failed");

    // --- guarded start promotion -------------------------------------------
    // `mark_instance_started` is what a detached launch uses to stamp `running`
    // *after* spawning the run. It must promote a not-yet-started instance and
    // refuse to touch one that already parked, or a workflow that suspends
    // faster than its launcher returns gets resurrected as `running` with no
    // process behind it — which the container monitor then fails as a crash.
    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Suspended, None)
        .await
        .expect("update_instance_status suspended failed (start guard setup)");
    let promoted_parked = backend
        .mark_instance_started(&instance_id, Utc::now())
        .await
        .expect("mark_instance_started failed (suspended)");
    assert!(
        !promoted_parked,
        "a suspended instance must not be promoted back to running"
    );
    let parked = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed (start guard)")
        .expect("instance must still exist");
    assert_eq!(
        parked.status,
        CoreInstanceStatus::Suspended,
        "the guarded promotion must leave a parked instance untouched"
    );

    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
        .await
        .expect("update_instance_status running failed (start guard reset)");
    let promoted_running = backend
        .mark_instance_started(&instance_id, Utc::now())
        .await
        .expect("mark_instance_started failed (running)");
    assert!(
        promoted_running,
        "an instance still in a pre-run state must be promoted"
    );

    // Hand the next section the `suspended` instance it expects.
    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Suspended, None)
        .await
        .expect("update_instance_status suspended failed (start guard teardown)");

    // --- batch claim (select and claim in one step) -------------------------
    // `claim_sleeping_instances_due` is what the wake scheduler uses once it
    // polls back-to-back: selecting and claiming have to happen together, or
    // overlapping polls keep re-selecting rows whose claim has not landed.
    // Re-arm the instance, then assert the batch call both returns it and
    // takes it out of the candidate set in one go.
    backend
        .set_instance_sleep(&instance_id, Utc::now() - Duration::seconds(60))
        .await
        .expect("set_instance_sleep failed (batch claim re-arm)");

    let claimed_batch = backend
        .claim_sleeping_instances_due(50, Utc::now() + chrono::Duration::seconds(120))
        .await
        .expect("claim_sleeping_instances_due failed");
    assert!(
        claimed_batch.iter().any(|r| r.instance_id == instance_id),
        "a due instance must be returned by the batch claim"
    );

    // Claimed means claimed: the row is gone from the due set, and a
    // subsequent single claim must lose, exactly as if the per-row claim had
    // run. This is the double-launch guarantee the scheduler relies on.
    let due_after_batch = backend
        .get_sleeping_instances_due(50)
        .await
        .expect("get_sleeping_instances_due failed (after batch claim)");
    assert!(
        due_after_batch.iter().all(|r| r.instance_id != instance_id),
        "an instance claimed by the batch call must no longer be due to wake"
    );
    let claim_after_batch = backend
        .claim_sleeping_instance(&instance_id)
        .await
        .expect("claim_sleeping_instance (after batch) failed");
    assert!(
        !claim_after_batch,
        "the batch claim must already own the instance, so a later claim loses"
    );

    // A second batch call with nothing due must come back empty rather than
    // re-returning an already-claimed row.
    let empty_batch = backend
        .claim_sleeping_instances_due(50, Utc::now() + chrono::Duration::seconds(120))
        .await
        .expect("claim_sleeping_instances_due (drained) failed");
    assert!(
        empty_batch.iter().all(|r| r.instance_id != instance_id),
        "an already-claimed instance must not be returned again"
    );

    backend
        .clear_instance_sleep(&instance_id)
        .await
        .expect("clear_instance_sleep failed (after batch claim)");

    // --- listing ------------------------------------------------------------
    // The instance is `suspended` by this point, and a suspended instance
    // occupies no concurrency slot. Re-running it pins that: count while
    // parked, count again once it is back to `running`, and require the slot
    // to appear only in the second reading.
    let parked = backend
        .count_active_instances()
        .await
        .expect("count_active_instances (suspended) failed");
    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Running, None)
        .await
        .expect("update_instance_status running (re-run) failed");
    let active = backend
        .count_active_instances()
        .await
        .expect("count_active_instances (running) failed");
    assert_eq!(
        active,
        parked + 1,
        "a suspended instance must not hold a concurrency slot"
    );
    let listed = backend
        .list_instances(Some(tenant_id), None, 50, 0)
        .await
        .expect("list_instances failed");
    assert!(listed.iter().any(|r| r.instance_id == instance_id));

    // --- retry attempt ------------------------------------------------------
    backend
        .save_retry_attempt(&instance_id, checkpoint_id, 1, Some("transient-parity"))
        .await
        .expect("save_retry_attempt failed");

    // --- completion ---------------------------------------------------------
    backend
        .complete_instance(
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Completed)
                .with_output(b"{\"result\":42}")
                .with_checkpoint(checkpoint_id),
        )
        .await
        .expect("complete_instance failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after complete failed")
        .expect("instance must still exist post-complete");
    assert_eq!(record.status, CoreInstanceStatus::Completed);

    // `output` and `error` are REPLACED, not merged: a transition that carries
    // no output clears whatever was there. The fields that do merge are
    // `termination_reason`, `exit_code`, `stderr` and `checkpoint_id`, which a
    // later transition must not erase. Two backends disagreed on this until it
    // was pinned here.
    backend
        .complete_instance(
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Failed)
                .with_error("boom")
                .with_termination("crashed", Some(137)),
        )
        .await
        .expect("complete_instance (replace) failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after replace failed")
        .expect("instance must still exist");
    assert_eq!(
        record.output, None,
        "a transition carrying no output must clear the previous one"
    );
    assert_eq!(record.error.as_deref(), Some("boom"));
    assert_eq!(
        record.checkpoint_id.as_deref(),
        Some(checkpoint_id),
        "checkpoint_id merges, so a later transition must not erase it"
    );
    assert_eq!(record.termination_reason.as_deref(), Some("crashed"));
    assert_eq!(record.exit_code, Some(137));

    // The merging fields keep their value when the next transition omits them.
    backend
        .complete_instance(CompleteInstanceParams::new(
            &instance_id,
            CoreInstanceStatus::Failed,
        ))
        .await
        .expect("complete_instance (merge) failed");
    let record = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after merge failed")
        .expect("instance must still exist");
    assert_eq!(
        record.termination_reason.as_deref(),
        Some("crashed"),
        "termination_reason merges: omitting it must not clear it"
    );
    assert_eq!(record.exit_code, Some(137), "exit_code merges");

    // --- retention sweep ----------------------------------------------------
    // The instance finished moments ago; using a slightly-future cutoff
    // guarantees it appears in the terminal sweep. An empty-list delete
    // is a no-op (returns 0).
    let empty_deleted = backend
        .delete_instances_batch(&[])
        .await
        .expect("delete_instances_batch with empty slice failed");
    assert_eq!(empty_deleted, 0);

    // The sweep returns the OLDEST terminal instances first, and this one was
    // just completed, so it sorts last. A limit near the number of terminal
    // rows already in the store would exclude it for reasons that have
    // nothing to do with the sweep working — the lib tests share a store
    // and it accumulates. Ask for more than it can plausibly hold instead, and
    // say so if it is ever hit.
    let cutoff = Utc::now() + Duration::seconds(60);
    let sweep_limit = 100_000;
    let terminal = backend
        .get_terminal_instances_older_than(cutoff, sweep_limit)
        .await
        .expect("get_terminal_instances_older_than failed");
    assert!(
        terminal.iter().any(|id| id == &instance_id),
        "completed instance must appear in terminal sweep before cutoff \
         (swept {} rows against a limit of {sweep_limit}; if those are equal \
         the limit, not the sweep, is what excluded it)",
        terminal.len()
    );

    let deleted = backend
        .delete_instances_batch(std::slice::from_ref(&instance_id))
        .await
        .expect("delete_instances_batch failed");
    assert_eq!(deleted, 1, "exactly one instance should be deleted");

    let post_delete = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance after delete failed");
    assert!(
        post_delete.is_none(),
        "instance row must be gone after delete_instances_batch"
    );

    // --- health -------------------------------------------------------------
    assert!(backend.health_check().await.expect("health_check failed"));
}
