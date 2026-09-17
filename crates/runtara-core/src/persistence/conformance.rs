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
    CheckpointRecord, CompleteInstanceParams, EventRecord, EventVocabulary, EventVocabularySpec,
    InstanceRecord, ListEventsFilter, ListPairedRecordsFilter, PairedRecordStatus, Persistence,
};

/// Assert a page of checkpoints descends by `(created_at, checkpoint_id)`,
/// comparing the id bytewise.
///
/// The ids are unique within an instance, so the order is total and each
/// neighbouring pair must be strictly decreasing.
///
/// This checks the primary key of the sort, not the tie-break. `save_checkpoint`
/// takes no timestamp — the backend supplies its own — so no fixture written
/// against this trait can make two rows share a `created_at`, which leaves the
/// id clause here permanently unreached. Pinning the tie-break means forcing a
/// tie through a backend's own storage, so it belongs in that backend's tests:
/// see `checkpoints_sharing_a_timestamp_break_the_tie_bytewise` in
/// `runtara-store-postgres`, where the database collation makes the byte order
/// a real requirement rather than an obvious one.
fn assert_checkpoints_ordered(page: &[CheckpointRecord]) {
    for pair in page.windows(2) {
        let (newer, older) = (&pair[0], &pair[1]);
        assert!(
            (newer.created_at, &newer.checkpoint_id) > (older.created_at, &older.checkpoint_id),
            "checkpoints must descend by (created_at, checkpoint_id), but {} precedes {}",
            newer.checkpoint_id,
            older.checkpoint_id
        );
    }
}

/// Assert a page of instances descends by `(created_at, instance_id)`,
/// comparing the id bytewise.
///
/// The ids are unique, so the order is total and each neighbouring pair must
/// be strictly decreasing.
///
/// Unlike [`assert_checkpoints_ordered`], the id clause here is reachable
/// through the trait: `register_instance` takes no timestamp, and a store
/// stamping from a microsecond-resolution wall clock ties two of four
/// back-to-back registrations about one run in five (measured on macOS).
/// Reachable is not the same as reliable, though, so what actually pins the
/// tie-break is a forced tie in each backend's own tests —
/// `in_memory_instances_sharing_a_timestamp_break_the_tie_bytewise` here and
/// `instances_sharing_a_timestamp_break_the_tie_bytewise` in
/// `runtara-store-postgres`, where the database collation makes the byte
/// order a real requirement rather than an obvious one.
fn assert_instances_ordered(page: &[InstanceRecord]) {
    for pair in page.windows(2) {
        let (newer, older) = (&pair[0], &pair[1]);
        assert!(
            (newer.created_at, &newer.instance_id) > (older.created_at, &older.instance_id),
            "instances must descend by (created_at, instance_id), but {} precedes {}",
            newer.instance_id,
            older.instance_id
        );
    }
}

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
    // Park it the way a drain force-stop does, through `complete_instance`,
    // which is what stamps the terminal fields. A bare status write to
    // `suspended` leaves them unset, and then the promotion assertions below
    // only prove that a field nothing ever wrote is still empty.
    backend
        .complete_instance(
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Suspended)
                .with_termination("sleeping", None),
        )
        .await
        .expect("suspend failed");
    let before = backend
        .get_instance(&instance_id)
        .await
        .expect("get_instance failed")
        .expect("instance should exist");
    assert_eq!(before.status, CoreInstanceStatus::Suspended);
    assert!(
        before.finished_at.is_some(),
        "precondition: parking an instance must stamp finished_at"
    );
    assert!(
        before.termination_reason.is_some(),
        "precondition: parking an instance must stamp termination_reason"
    );

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
    // The stamps above describe a run that is no longer over. Carrying them
    // into `running` puts `finished_at` before `started_at`, which renders as
    // a negative duration.
    assert!(
        promoted.finished_at.is_none(),
        "promoting a parked instance must clear the stale finished_at"
    );
    assert!(
        promoted.termination_reason.is_none(),
        "promoting a parked instance must clear the stale termination_reason"
    );

    // --- update status → running -------------------------------------------
    // Same clear, asserted on the raw write rather than through
    // `mark_instance_running`, so a backend that overrides the promotion
    // helpers is still pinned here.
    backend
        .complete_instance(
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Suspended)
                .with_termination("sleeping", None),
        )
        .await
        .expect("re-park before the raw status write failed");
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
    assert!(
        record.finished_at.is_none(),
        "a status write that stamps started_at must clear the stale finished_at"
    );
    assert!(
        record.termination_reason.is_none(),
        "a status write that stamps started_at must clear the stale termination_reason"
    );

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

    // --- checkpoint ordering ------------------------------------------------
    // `list_checkpoints` pages with limit/offset, so it needs a *total* order or
    // `offset` walks a set the store is free to re-shuffle between calls. The
    // contract is `(created_at, checkpoint_id)` descending. These ids sort
    // lexicographically in write order, so the expected sequence holds whether
    // or not two writes land in the same clock tick.
    for id in ["ckpt-2", "ckpt-3"] {
        backend
            .save_checkpoint(&instance_id, id, id.as_bytes())
            .await
            .expect("save_checkpoint failed (ordering fixture)");
    }

    let ordered = backend
        .list_checkpoints(&instance_id, None, 50, 0, None, None)
        .await
        .expect("list_checkpoints for ordering failed");
    let ids: Vec<String> = ordered.iter().map(|c| c.checkpoint_id.clone()).collect();
    assert_eq!(
        ids,
        ["ckpt-3", "ckpt-2", "ckpt-1"],
        "list_checkpoints must return newest first"
    );
    assert_checkpoints_ordered(&ordered);

    // Offset pagination must tile that same order. This is the symptom an
    // unstated order produces: matching totals, mismatched pages.
    let mut one_at_a_time = Vec::new();
    for offset in 0..3 {
        let page = backend
            .list_checkpoints(&instance_id, None, 1, offset, None, None)
            .await
            .expect("single-row page of list_checkpoints failed");
        assert_eq!(
            page.len(),
            1,
            "offset {offset} over three checkpoints must yield a row"
        );
        one_at_a_time.push(page[0].checkpoint_id.clone());
    }
    assert_eq!(
        one_at_a_time, ids,
        "paging one row at a time must tile the unpaged order, not a second one"
    );

    let first_page = backend
        .list_checkpoints(&instance_id, None, 2, 0, None, None)
        .await
        .expect("first page of list_checkpoints failed");
    let second_page = backend
        .list_checkpoints(&instance_id, None, 2, 2, None, None)
        .await
        .expect("second page of list_checkpoints failed");
    let tiled: Vec<String> = first_page
        .iter()
        .chain(second_page.iter())
        .map(|c| c.checkpoint_id.clone())
        .collect();
    assert_eq!(
        tiled, ids,
        "two-row pages must tile the unpaged order without skipping or repeating a row"
    );

    // A re-save restamps `created_at`. Replay re-saves every key it already
    // wrote, so a backend that keeps the original stamp pages a resumed instance
    // in a different order than one that does not — with the sort above still in
    // place, which is what makes this worth pinning here. Compared against the
    // newest existing stamp rather than by position: a backend that fails to
    // restamp leaves this checkpoint strictly older, while a tie between two
    // same-tick writes is conformant.
    let newest_before_resave = ordered[0].created_at;
    backend
        .save_checkpoint(&instance_id, checkpoint_id, &refreshed_state)
        .await
        .expect("re-saving for the restamp check failed");
    let restamped = backend
        .list_checkpoints(&instance_id, Some(checkpoint_id), 50, 0, None, None)
        .await
        .expect("list_checkpoints after re-save failed");
    assert_eq!(restamped.len(), 1, "a re-save must not add a second row");
    assert!(
        restamped[0].created_at >= newest_before_resave,
        "a re-save must restamp created_at: {checkpoint_id} still dates from before \
         the checkpoints written after it, so it pages as the oldest"
    );
    let reordered = backend
        .list_checkpoints(&instance_id, None, 50, 0, None, None)
        .await
        .expect("list_checkpoints after re-save failed");
    assert_checkpoints_ordered(&reordered);

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
        .acknowledge_signal(&instance_id, &pending.command_id, pending.signal_type)
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
    let first_signal_id = backend
        .put_custom_signal(&instance_id, checkpoint_id, &custom_payload)
        .await
        .expect("put_custom_signal failed");
    let taken = backend
        .get_custom_signal(&instance_id, checkpoint_id)
        .await
        .expect("get_custom_signal failed")
        .expect("custom signal should be readable");
    assert_eq!(taken.checkpoint_id, checkpoint_id);
    // Reads are non-destructive: a replayed WaitForSignal re-reads the same
    // signal after a drain/resume, so a second read returns the row again
    // rather than None. Instance deletion reclaims the signal.
    let taken_again = backend
        .get_custom_signal(&instance_id, checkpoint_id)
        .await
        .expect("get_custom_signal second call failed")
        .expect("custom signal must remain re-readable (non-destructive)");
    assert_eq!(taken_again.checkpoint_id, checkpoint_id);
    assert_eq!(taken_again.payload, taken.payload);
    assert_eq!(taken.signal_id, first_signal_id);
    assert_eq!(taken_again.signal_id, first_signal_id);
    assert_ne!(first_signal_id, checkpoint_id);
    // Identical retries create a new value identity: no implicit deduplication.
    let retry_id = backend
        .put_custom_signal(&instance_id, checkpoint_id, &custom_payload)
        .await
        .unwrap();
    assert_ne!(retry_id, first_signal_id);
    let replacement = b"replacement";
    let replacement_id = backend
        .put_custom_signal(&instance_id, checkpoint_id, replacement)
        .await
        .unwrap();
    assert_ne!(replacement_id, retry_id);
    for _ in 0..2 {
        let value = backend
            .get_custom_signal(&instance_id, checkpoint_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(value.signal_id, replacement_id);
        assert_eq!(value.payload.as_deref(), Some(replacement.as_slice()));
    }
    assert!(
        backend
            .get_custom_signal(&instance_id, "different-address")
            .await
            .unwrap()
            .is_none()
    );

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

    // Restore running for the independent sleep contract after cancellation.
    backend
        .update_instance_status(&instance_id, CoreInstanceStatus::Running, None)
        .await
        .unwrap();

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

    // Sequential claims only prove the *second* caller sees the first one's
    // write. Overlapping callers are a separate question, and
    // `run_concurrent_claim_sequence` is where it is asked.

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

    // --- instance listing order ---------------------------------------------
    // `list_instances` pages with limit/offset, so it needs a *total* order or
    // `offset` walks a set the store is free to re-shuffle between calls. The
    // contract is `(created_at, instance_id)` descending, the id compared
    // bytewise.
    //
    // Its own tenant, unique per run: these assertions are about the exact
    // contents of a listing, and `tenant_id` above is a fixed string that
    // gains a row on every run against a persistent database.
    let order_run = Uuid::new_v4().to_string();
    let order_tenant = format!("conformance-order-{order_run}");
    // Registered in ascending *byte* order, so the expected listing is the
    // exact reverse either way: with distinct timestamps it is creation order
    // reversed, and under a tie it is the byte order the contract falls back
    // to. It has to work both ways, because whether these four tie is a
    // property of the store's clock rather than of this fixture — a
    // microsecond-resolution one ties about one run in five.
    //
    // That also means this block cannot be what pins the tie-break; a forced
    // tie in each backend's own tests does (see `assert_instances_ordered`).
    // The suffixes are still the collation-sensitive ones those tests use:
    // under `en_US.utf8` punctuation and case are weak, ranking them
    // `Step-A, stepA, step_a, step-a` where the bytes rank them
    // `step_a, stepA, step-a, Step-A` — `_` (0x5F) over `A` (0x41) over `-`
    // (0x2D), and lowercase `s` (0x73) over capital `S` (0x53). So on the
    // runs that do tie, a backend sorting under its collation disagrees with
    // this expectation rather than matching it by luck.
    let ordered_ids: Vec<String> = ["Step-A", "step-a", "stepA", "step_a"]
        .iter()
        .map(|suffix| format!("{order_run}-{suffix}"))
        .collect();
    for id in &ordered_ids {
        backend
            .register_instance(id, &order_tenant)
            .await
            .expect("register_instance failed (ordering fixture)");
    }

    let listed_order = backend
        .list_instances(Some(&order_tenant), None, 50, 0)
        .await
        .expect("list_instances for ordering failed");
    let listed_order_ids: Vec<String> =
        listed_order.iter().map(|i| i.instance_id.clone()).collect();
    let newest_first: Vec<String> = ordered_ids.iter().rev().cloned().collect();
    assert_eq!(
        listed_order_ids, newest_first,
        "list_instances must return newest first, ties broken on the id compared bytewise"
    );
    assert_instances_ordered(&listed_order);

    // Offset pagination must tile that same order. This is the symptom an
    // unstated order produces: matching totals, mismatched pages.
    let mut one_at_a_time = Vec::new();
    for offset in 0..4 {
        let page = backend
            .list_instances(Some(&order_tenant), None, 1, offset)
            .await
            .expect("single-row page of list_instances failed");
        assert_eq!(
            page.len(),
            1,
            "offset {offset} over four instances must yield a row"
        );
        one_at_a_time.push(page[0].instance_id.clone());
    }
    assert_eq!(
        one_at_a_time, listed_order_ids,
        "paging one row at a time must tile the unpaged order, not a second one"
    );

    let first_page = backend
        .list_instances(Some(&order_tenant), None, 2, 0)
        .await
        .expect("first page of list_instances failed");
    let second_page = backend
        .list_instances(Some(&order_tenant), None, 2, 2)
        .await
        .expect("second page of list_instances failed");
    let tiled: Vec<String> = first_page
        .iter()
        .chain(second_page.iter())
        .map(|i| i.instance_id.clone())
        .collect();
    assert_eq!(
        tiled, listed_order_ids,
        "two-row pages must tile the unpaged order without skipping or repeating a row"
    );

    // The status filter narrows the set; it does not get to reorder it. All
    // four are still `pending`, so this must return the same sequence.
    let filtered = backend
        .list_instances(
            Some(&order_tenant),
            Some(CoreInstanceStatus::Pending),
            50,
            0,
        )
        .await
        .expect("status-filtered list_instances failed");
    let filtered_ids: Vec<String> = filtered.iter().map(|i| i.instance_id.clone()).collect();
    assert_eq!(
        filtered_ids, listed_order_ids,
        "narrowing by status must preserve the order, not substitute another one"
    );

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

/// Receipt identity, cancellation precedence, and idempotent lifecycle transitions.
/// Run unchanged against each persistence implementation.
pub async fn run_lifecycle_command_sequence<P: Persistence>(backend: &P) {
    terminal_cancel_races(backend).await;
    use crate::domain::{InstanceStatus as Status, SignalType as Kind};
    let id = Uuid::new_v4().to_string();
    backend
        .register_instance(&id, "command-contract")
        .await
        .unwrap();
    backend
        .update_instance_status(&id, Status::Running, None)
        .await
        .unwrap();
    backend
        .insert_signal(&id, Kind::Pause, b"first")
        .await
        .unwrap();
    let first = backend.get_pending_signal(&id).await.unwrap().unwrap();
    backend
        .insert_signal(&id, Kind::Pause, b"replacement")
        .await
        .unwrap();
    let second = backend.get_pending_signal(&id).await.unwrap().unwrap();
    assert_ne!(
        first.command_id, second.command_id,
        "same-kind commands need distinct receipts"
    );
    assert!(
        !backend
            .acknowledge_signal(&id, &first.command_id, Kind::Pause)
            .await
            .unwrap()
    );
    assert!(
        !backend
            .acknowledge_signal(&id, &second.command_id, Kind::Cancel)
            .await
            .unwrap()
    );
    assert_eq!(
        backend.get_instance(&id).await.unwrap().unwrap().status,
        Status::Running
    );
    assert_eq!(
        backend
            .get_pending_signal(&id)
            .await
            .unwrap()
            .unwrap()
            .command_id,
        second.command_id
    );
    assert!(
        backend
            .acknowledge_signal(&id, &second.command_id, Kind::Pause)
            .await
            .unwrap()
    );
    assert_eq!(
        backend.get_instance(&id).await.unwrap().unwrap().status,
        Status::Suspended
    );
    assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
    // A retry after resume must not apply the old pause again.
    backend
        .update_instance_status(&id, Status::Running, None)
        .await
        .unwrap();
    assert!(
        backend
            .acknowledge_signal(&id, &second.command_id, Kind::Pause)
            .await
            .unwrap()
    );
    assert_eq!(
        backend.get_instance(&id).await.unwrap().unwrap().status,
        Status::Running
    );
    let events = backend
        .list_events(&id, &ListEventsFilter::default(), 100, 0)
        .await
        .unwrap();
    assert_eq!(
        events.len(),
        1,
        "pause and its retry record exactly one suspension event"
    );
    assert_eq!(events[0].event_type, crate::domain::EventType::Suspended);
    backend
        .insert_signal(&id, Kind::Shutdown, b"")
        .await
        .unwrap();
    let shutdown = backend.get_pending_signal(&id).await.unwrap().unwrap();
    assert!(
        backend
            .acknowledge_signal(&id, &shutdown.command_id, Kind::Shutdown)
            .await
            .unwrap()
    );
    let suspended = backend.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(suspended.status, Status::Suspended);
    assert_eq!(
        suspended.termination_reason.as_deref(),
        Some("shutdown_requested")
    );
    assert!(suspended.sleep_until.is_some());
    backend
        .insert_signal(&id, Kind::Cancel, b"cancel")
        .await
        .unwrap();
    let cancel = backend.get_pending_signal(&id).await.unwrap().unwrap();
    for kind in [Kind::Pause, Kind::Shutdown, Kind::Cancel] {
        backend.insert_signal(&id, kind, b"later").await.unwrap();
        assert_eq!(
            backend
                .get_pending_signal(&id)
                .await
                .unwrap()
                .unwrap()
                .command_id,
            cancel.command_id
        );
    }
    assert!(
        !backend
            .acknowledge_signal(&id, &shutdown.command_id, Kind::Shutdown)
            .await
            .unwrap()
    );
    assert!(
        backend
            .acknowledge_signal(&id, &cancel.command_id, Kind::Cancel)
            .await
            .unwrap()
    );
    let cancelled = backend.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.finished_at.is_some());
    assert!(cancelled.sleep_until.is_none());
    assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
    backend.insert_signal(&id, Kind::Pause, b"").await.unwrap();
    let late = backend.get_pending_signal(&id).await.unwrap().unwrap();
    assert!(
        !backend
            .acknowledge_signal(&id, &late.command_id, Kind::Pause)
            .await
            .unwrap()
    );
    assert_eq!(
        backend.get_instance(&id).await.unwrap().unwrap().status,
        Status::Cancelled
    );
}

/// Parked cancellation is atomic, repeatable, and recoverable without a deadline.
pub async fn run_parked_cancellation_sequence<P: Persistence>(backend: &P) {
    use crate::domain::{InstanceStatus as Status, SignalType as Kind};
    for deadline in [None, Some(Utc::now() + Duration::hours(24))] {
        let id = Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "park-contract")
            .await
            .unwrap();
        backend
            .update_instance_status(&id, Status::Running, None)
            .await
            .unwrap();
        backend.insert_signal(&id, Kind::Cancel, b"").await.unwrap();
        assert!(
            backend
                .cancel_suspended_instances(Some(&id), 1)
                .await
                .unwrap()
                .is_empty(),
            "a running guest retains its command"
        );
        assert!(backend.get_pending_signal(&id).await.unwrap().is_some());
        // Model a cancel arriving just before the guest parks.
        backend
            .update_instance_status(&id, Status::Suspended, None)
            .await
            .unwrap();
        if let Some(deadline) = deadline {
            backend.set_instance_sleep(&id, deadline).await.unwrap();
        }
        let cancelled = backend
            .cancel_suspended_instances(Some(&id), 1)
            .await
            .unwrap();
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].instance_id, id);
        assert_eq!(cancelled[0].tenant_id, "park-contract");
        let instance = backend.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(instance.status, Status::Cancelled);
        assert!(instance.finished_at.is_some());
        assert!(instance.sleep_until.is_none());
        assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
        assert!(
            backend
                .cancel_suspended_instances(Some(&id), 1)
                .await
                .unwrap()
                .is_empty()
        );
        // A park or wake retry that lost the race cannot put its deadline back.
        backend
            .set_instance_sleep(&id, Utc::now() + Duration::hours(1))
            .await
            .unwrap();
        assert!(
            backend
                .get_instance(&id)
                .await
                .unwrap()
                .unwrap()
                .sleep_until
                .is_none()
        );
    }
    let id = Uuid::new_v4().to_string();
    backend
        .register_instance(&id, "park-recovery")
        .await
        .unwrap();
    backend
        .update_instance_status(&id, Status::Suspended, None)
        .await
        .unwrap();
    backend.insert_signal(&id, Kind::Pause, b"").await.unwrap();
    assert!(
        backend
            .cancel_suspended_instances(Some(&id), 1)
            .await
            .unwrap()
            .is_empty()
    );
    backend.insert_signal(&id, Kind::Cancel, b"").await.unwrap();
    assert!(
        backend
            .cancel_suspended_instances(None, 0)
            .await
            .unwrap()
            .is_empty()
    );
    let recovered = backend
        .cancel_suspended_instances(None, 1000)
        .await
        .unwrap();
    assert!(
        recovered.iter().any(|instance| instance.instance_id == id),
        "recovery discovers parked cancellation without a sleep deadline"
    );
}

/// Observable lifecycle matrix shared by every backend. Expected values are
/// specified independently of the pure policy implementation.
pub async fn run_lifecycle_policy_matrix<P: Persistence>(backend: &P) {
    use crate::{
        domain::{InstanceStatus as S, SignalType as K},
        lifecycle::{Decision, ParkReason, ParkRequest},
    };
    let statuses = [
        S::Pending,
        S::Running,
        S::Suspended,
        S::Completed,
        S::Failed,
        S::Cancelled,
    ];
    let kinds = [K::Cancel, K::Pause, K::Shutdown];
    for status in statuses {
        for kind in kinds {
            let id = Uuid::new_v4().to_string();
            backend
                .register_instance(&id, "policy-matrix")
                .await
                .unwrap();
            backend
                .complete_instance(
                    CompleteInstanceParams::new(&id, status)
                        .with_output(b"previous-output")
                        .with_error("previous-error")
                        .with_termination("crashed", Some(91))
                        .with_checkpoint("previous-checkpoint"),
                )
                .await
                .unwrap();
            backend.insert_signal(&id, kind, b"payload").await.unwrap();
            let command = backend.get_pending_signal(&id).await.unwrap().unwrap();
            let before = backend.get_instance(&id).await.unwrap().unwrap();
            let decision = backend
                .apply_lifecycle_command(&id, &command.command_id, kind)
                .await
                .unwrap();
            let rejects = matches!(status, S::Completed | S::Failed | S::Cancelled);
            if rejects {
                assert_eq!(decision, Decision::Rejected);
            } else {
                assert!(matches!(decision, Decision::Applied(_)));
            }
            let after = backend.get_instance(&id).await.unwrap().unwrap();
            let expected_status = if rejects {
                status
            } else {
                match kind {
                    K::Cancel => S::Cancelled,
                    K::Pause | K::Shutdown => S::Suspended,
                }
            };
            assert_eq!(after.status, expected_status, "{status:?} + {kind:?}");
            let reason = if rejects {
                Some("crashed")
            } else {
                match kind {
                    K::Pause => None,
                    K::Shutdown => Some("shutdown_requested"),
                    _ => Some("crashed"),
                }
            };
            assert_eq!(after.termination_reason.as_deref(), reason);
            assert_eq!(after.output, before.output);
            assert_eq!(after.error, before.error);
            assert_eq!(after.exit_code, before.exit_code);
            assert_eq!(after.checkpoint_id, before.checkpoint_id);
            assert_eq!(after.sleep_until.is_some(), !rejects && kind == K::Shutdown);
            assert_eq!(
                after.wake_reason,
                if !rejects && kind == K::Shutdown {
                    Some(crate::domain::WakeReason::Recovery)
                } else {
                    before.wake_reason
                }
            );
            let event_count = backend
                .count_events(&id, &ListEventsFilter::default())
                .await
                .unwrap();
            assert_eq!(
                event_count,
                i64::from(!rejects && matches!(kind, K::Pause | K::Shutdown))
            );
            if !rejects {
                assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
                assert_eq!(
                    backend
                        .apply_lifecycle_command(&id, &command.command_id, kind)
                        .await
                        .unwrap(),
                    Decision::AlreadyApplied
                );
                let repeated = backend.get_instance(&id).await.unwrap().unwrap();
                assert_eq!(repeated.finished_at, after.finished_at);
                assert_eq!(repeated.sleep_until, after.sleep_until);
                assert_eq!(
                    backend
                        .count_events(&id, &ListEventsFilter::default())
                        .await
                        .unwrap(),
                    event_count
                );
            } else {
                assert_eq!(
                    backend
                        .get_pending_signal(&id)
                        .await
                        .unwrap()
                        .unwrap()
                        .command_id,
                    command.command_id
                );
            }
            backend.delete_instances_batch(&[id]).await.unwrap();
        }
        for reason in [ParkReason::Timer, ParkReason::Signal] {
            for with_deadline in [false, true] {
                let id = Uuid::new_v4().to_string();
                backend
                    .register_instance(&id, "park-policy-matrix")
                    .await
                    .unwrap();
                backend
                    .complete_instance(
                        CompleteInstanceParams::new(&id, status)
                            .with_output(b"old-result")
                            .with_error("old-error")
                            .with_termination("crashed", Some(91))
                            .with_checkpoint("saved"),
                    )
                    .await
                    .unwrap();
                backend.insert_signal(&id, K::Cancel, b"").await.unwrap();
                let receipt = backend.get_pending_signal(&id).await.unwrap().unwrap();
                let before = backend.get_instance(&id).await.unwrap().unwrap();
                let deadline = with_deadline.then(|| Utc::now() + Duration::hours(1));
                let result = backend
                    .park_instance(&id, ParkRequest { reason, deadline })
                    .await
                    .unwrap();
                let after = backend.get_instance(&id).await.unwrap().unwrap();
                if status == S::Running {
                    assert!(matches!(result, Decision::Applied(_)));
                    assert_eq!(after.status, S::Suspended);
                    assert_eq!(
                        after.termination_reason.as_deref(),
                        Some(if reason == ParkReason::Timer {
                            "sleeping"
                        } else {
                            "waiting_signal"
                        })
                    );
                    assert_eq!(
                        after.sleep_until.map(|d| d.timestamp_millis()),
                        deadline.map(|d| d.timestamp_millis())
                    );
                    assert!(after.finished_at.is_some());
                    assert!(after.output.is_none() && after.error.is_none());
                } else {
                    assert_eq!(result, Decision::Rejected);
                    assert_eq!(after.status, before.status);
                    assert_eq!(after.finished_at, before.finished_at);
                    assert_eq!(after.termination_reason, before.termination_reason);
                    assert_eq!(after.sleep_until, before.sleep_until);
                    assert_eq!(after.output, before.output);
                    assert_eq!(after.error, before.error);
                }
                assert_eq!(after.checkpoint_id, before.checkpoint_id);
                assert_eq!(after.exit_code, before.exit_code);
                assert_eq!(
                    backend
                        .get_pending_signal(&id)
                        .await
                        .unwrap()
                        .unwrap()
                        .command_id,
                    receipt.command_id
                );
                assert_eq!(
                    backend
                        .count_events(&id, &ListEventsFilter::default())
                        .await
                        .unwrap(),
                    0
                );
                backend.delete_instances_batch(&[id]).await.unwrap();
            }
        }
    }
}

/// Wake causes survive claiming and re-launching, and never become guest commands.
pub async fn run_wake_reason_sequence<P: Persistence>(backend: &P) {
    use crate::domain::{InstanceStatus, WakeReason};
    for reason in [
        WakeReason::Timer,
        WakeReason::CustomSignal,
        WakeReason::ManualResume,
        WakeReason::Recovery,
    ] {
        let id = uuid::Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "wake-contract")
            .await
            .unwrap();
        backend
            .update_instance_status(&id, InstanceStatus::Suspended, None)
            .await
            .unwrap();
        backend
            .schedule_wake(&id, Utc::now() - Duration::seconds(1), reason)
            .await
            .unwrap();
        let due = backend.get_sleeping_instances_due(1000).await.unwrap();
        assert_eq!(
            due.iter()
                .find(|row| row.instance_id == id)
                .unwrap()
                .wake_reason,
            Some(reason)
        );
        assert!(backend.claim_sleeping_instance(&id).await.unwrap());
        backend
            .mark_instance_running(&id, Utc::now())
            .await
            .unwrap();
        let running = backend.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(running.wake_reason, Some(reason));
        assert!(backend.get_pending_signal(&id).await.unwrap().is_none());
        backend
            .update_instance_status(&id, InstanceStatus::Completed, None)
            .await
            .unwrap();
        backend
            .schedule_wake(&id, Utc::now(), reason)
            .await
            .unwrap();
        let ended = backend.get_instance(&id).await.unwrap().unwrap();
        assert!(ended.sleep_until.is_none());
        assert!(ended.wake_reason.is_none());
        backend.delete_instances_batch(&[id]).await.unwrap();
    }
}

/// Atomic isolated-invocation contract cases shared by all capable backends.
pub mod invocations;

// Exercise both orderings and real concurrent writers against each backend.
// Expected outcomes are independent of the lifecycle policy implementation.
async fn terminal_cancel_races<P: Persistence>(backend: &P) {
    use crate::domain::{InstanceStatus as S, SignalType as K};
    for terminal in [S::Completed, S::Failed, S::Cancelled] {
        for cancel_before_completion in [false, true] {
            let id = Uuid::new_v4().to_string();
            backend
                .register_instance(&id, "terminal-cancel")
                .await
                .unwrap();
            backend
                .update_instance_status(&id, S::Running, None)
                .await
                .unwrap();
            if cancel_before_completion {
                backend
                    .insert_signal(&id, K::Cancel, b"request")
                    .await
                    .unwrap();
            }
            backend
                .complete_instance(
                    CompleteInstanceParams::new(&id, terminal)
                        .if_running()
                        .with_output(b"accepted output")
                        .with_error("accepted error")
                        .with_termination("crashed", Some(7)),
                )
                .await
                .unwrap();
            let before = backend.get_instance(&id).await.unwrap().unwrap();
            if !cancel_before_completion {
                backend
                    .insert_signal(&id, K::Cancel, b"request")
                    .await
                    .unwrap();
            }
            let command = backend.get_pending_signal(&id).await.unwrap().unwrap();
            for _ in 0..2 {
                assert!(
                    !backend
                        .acknowledge_signal(&id, &command.command_id, K::Cancel)
                        .await
                        .unwrap()
                );
            }
            let after = backend.get_instance(&id).await.unwrap().unwrap();
            assert_eq!(after.status, terminal);
            assert_eq!(after.output, before.output);
            assert_eq!(after.error, before.error);
            assert_eq!(after.finished_at, before.finished_at);
            assert_eq!(after.termination_reason, before.termination_reason);
            assert_eq!(after.exit_code, before.exit_code);
            let pending = backend.get_pending_signal(&id).await.unwrap().unwrap();
            assert_eq!(pending.command_id, command.command_id);
            assert!(pending.acknowledged_at.is_none());
        }
    }
    // Pin cancellation-first as well as completion-first, rather than relying
    // on the scheduler to produce both winners in the concurrent exercise.
    let id = Uuid::new_v4().to_string();
    backend
        .register_instance(&id, "cancel-first")
        .await
        .unwrap();
    backend
        .update_instance_status(&id, S::Running, None)
        .await
        .unwrap();
    backend
        .insert_signal(&id, K::Cancel, b"request")
        .await
        .unwrap();
    let command = backend.get_pending_signal(&id).await.unwrap().unwrap();
    assert!(
        backend
            .acknowledge_signal(&id, &command.command_id, K::Cancel)
            .await
            .unwrap()
    );
    let before = backend.get_instance(&id).await.unwrap().unwrap();
    assert!(
        !backend
            .complete_instance(
                CompleteInstanceParams::new(&id, S::Completed)
                    .if_running()
                    .with_output(b"late output")
            )
            .await
            .unwrap()
    );
    let after = backend.get_instance(&id).await.unwrap().unwrap();
    assert_eq!(after.status, S::Cancelled);
    assert_eq!(after.output, None);
    assert_eq!(after.finished_at, before.finished_at);
    assert!(
        backend
            .acknowledge_signal(&id, &command.command_id, K::Cancel)
            .await
            .unwrap()
    );
    for _ in 0..16 {
        let id = Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "concurrent-terminal-cancel")
            .await
            .unwrap();
        backend
            .update_instance_status(&id, S::Running, None)
            .await
            .unwrap();
        backend
            .insert_signal(&id, K::Cancel, b"request")
            .await
            .unwrap();
        let command = backend.get_pending_signal(&id).await.unwrap().unwrap();
        let params = CompleteInstanceParams::new(&id, S::Completed)
            .if_running()
            .with_output(b"result");
        let (completed, cancelled) = tokio::join!(
            backend.complete_instance(params),
            backend.acknowledge_signal(&id, &command.command_id, K::Cancel)
        );
        let completed = completed.unwrap();
        let cancelled = cancelled.unwrap();
        assert_ne!(completed, cancelled, "exactly one terminal transition wins");
        let result = backend.get_instance(&id).await.unwrap().unwrap();
        assert_eq!(
            result.status,
            if completed {
                S::Completed
            } else {
                S::Cancelled
            }
        );
        assert_eq!(
            result.output.as_deref(),
            if completed {
                Some(b"result".as_slice())
            } else {
                None
            }
        );
        assert_eq!(
            backend
                .acknowledge_signal(&id, &command.command_id, K::Cancel)
                .await
                .unwrap(),
            cancelled
        );
    }
}

/// Race real, parallel wakers for one due instance and require a single winner.
///
/// [`run_conformance_sequence`] claims twice in a row, which only proves the
/// second caller observes the first one's write. It says nothing about two
/// callers *overlapping*, and overlapping is the case
/// [`Persistence::claim_sleeping_instance`] exists for: a claim that reads a
/// claimable row and then clears it in a separate step lets every concurrent
/// caller read before any of them writes, and they all win. Each extra winner
/// is another launch of the same instance.
///
/// Takes an `Arc` and spawns, rather than polling futures concurrently on one
/// task: a claim whose steps never yield to the executor completes before the
/// next future is polled, and no amount of `join!` interleaves it. Run this on
/// a multi-threaded runtime — `#[tokio::test(flavor = "multi_thread")]` —
/// since a single-threaded one reintroduces exactly the serialization this is
/// trying to avoid.
pub async fn run_concurrent_claim_sequence<P: Persistence + 'static>(backend: std::sync::Arc<P>) {
    /// Enough contenders that a lost race is overwhelmingly likely to show up,
    /// while staying inside a small connection pool.
    const CONTENDERS: usize = 8;
    /// Repeats, because a race that only sometimes interleaves is still a race.
    const ROUNDS: usize = 20;

    let tenant_id = "conformance-tenant-concurrent";
    let mut rounds_won = 0usize;
    let mut claimed_instances: Vec<String> = Vec::with_capacity(ROUNDS);

    for round in 0..ROUNDS {
        let instance_id = Uuid::new_v4().to_string();
        backend
            .register_instance(&instance_id, tenant_id)
            .await
            .expect("register_instance failed (concurrent claim)");
        backend
            .update_instance_status(&instance_id, CoreInstanceStatus::Suspended, None)
            .await
            .expect("update_instance_status suspended failed (concurrent claim)");
        backend
            .set_instance_sleep(&instance_id, Utc::now() - Duration::seconds(30))
            .await
            .expect("set_instance_sleep failed (concurrent claim)");

        // Release every contender at once, so they reach the claim together
        // instead of in spawn order.
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(CONTENDERS));
        let mut contenders = Vec::with_capacity(CONTENDERS);
        for _ in 0..CONTENDERS {
            let backend = std::sync::Arc::clone(&backend);
            let gate = std::sync::Arc::clone(&gate);
            let instance_id = instance_id.clone();
            contenders.push(tokio::spawn(async move {
                gate.wait().await;
                backend.claim_sleeping_instance(&instance_id).await
            }));
        }

        let mut winners = 0;
        for contender in contenders {
            let claimed = contender
                .await
                .expect("claim task panicked")
                .expect("claim_sleeping_instance (concurrent) failed");
            if claimed {
                winners += 1;
            }
        }

        // Safety, asserted per round: never more than one winner. Each extra
        // winner is another launch of this instance.
        assert!(
            winners <= 1,
            "at most one of {CONTENDERS} concurrent claims may win (round {round}); \
             {winners} winners is {winners} launches of the same instance"
        );
        rounds_won += winners;
        claimed_instances.push(instance_id);
    }

    // Liveness, asserted once over the whole run: zero winners in a round is
    // legal — `claim_sleeping_instances_due` is a global claim with no tenant
    // filter, so a rival test sharing this database can take the row first —
    // but a backend whose claim *never* succeeds would satisfy the safety
    // assertion above in every round while waking nothing. This is what
    // separates the two.
    assert!(
        rounds_won > 0,
        "no round of {ROUNDS} produced a winner: this claim never succeeds, \
         so nothing would ever wake"
    );

    // Leave nothing behind: a claimed instance is `suspended` with no
    // `sleep_until`, which neither the retention sweep (terminal only) nor the
    // wake scan (`sleep_until` present) will ever collect.
    for instance_id in &claimed_instances {
        backend
            .update_instance_status(instance_id, CoreInstanceStatus::Completed, None)
            .await
            .expect("update_instance_status failed (concurrent claim cleanup)");
    }
    backend
        .delete_instances_batch(&claimed_instances)
        .await
        .expect("delete_instances_batch failed (concurrent claim cleanup)");
}

/// A batch claim must never expose a row without a wake deadline.
///
/// [`Persistence::claim_sleeping_instances_due`] leases rather than clears, and
/// the point of the lease is recovery: a row whose claimer dies becomes due
/// again on its own. That only holds if the row carries a deadline at *every*
/// instant. A claim assembled from a clear and a later re-stamp satisfies every
/// before-and-after assertion -- the deadline is there when you look afterwards
/// -- while still leaving a window in which the row is `suspended` with
/// `sleep_until = NULL`. That is the shape of a signal waiter, which no sweep
/// collects: the wake scan skips it for having no deadline, the retention sweep
/// for not being terminal. A process that dies inside that window strands its
/// whole batch permanently.
///
/// So this watches *during* the claim instead of after it. A reader samples the
/// rows continuously while the batch is claimed, and the claim is only correct
/// if the deadline-less state is never observable.
///
/// Takes an `Arc` and spawns for the same reason as
/// [`run_concurrent_claim_sequence`], and must likewise run on a multi-threaded
/// runtime: on a current-thread runtime the reader cannot be scheduled while
/// the claim is between its own steps, and the window closes for the wrong
/// reason.
pub async fn run_batch_claim_never_strands_sequence<P: Persistence + 'static>(
    backend: std::sync::Arc<P>,
) {
    /// Enough rows that a per-row claim has to loop, widening the window a
    /// reader can land in, while staying inside a small connection pool.
    const SLEEPERS: usize = 12;
    /// Bounded so a genuine strand fails the test rather than hanging it.
    const CLAIM_ROUNDS: usize = 10;
    /// Backdated far enough that these rows are the *oldest* due ones in the
    /// store. The claim is global and orders by `sleep_until` ascending, so
    /// rows seeded a few seconds back would sort behind any older backlog a
    /// shared database happens to hold -- and a bounded loop would then never
    /// reach them, failing the liveness assertion below against a backend that
    /// is working correctly, after leasing that entire backlog forward.
    const BACKDATE: i64 = 86_400;

    let tenant_id = "conformance-tenant-batch-lease";
    let mut sleepers = Vec::with_capacity(SLEEPERS);
    for _ in 0..SLEEPERS {
        let instance_id = Uuid::new_v4().to_string();
        backend
            .register_instance(&instance_id, tenant_id)
            .await
            .expect("register_instance failed (batch lease)");
        backend
            .update_instance_status(&instance_id, CoreInstanceStatus::Suspended, None)
            .await
            .expect("update_instance_status suspended failed (batch lease)");
        backend
            .set_instance_sleep(&instance_id, Utc::now() - Duration::seconds(BACKDATE))
            .await
            .expect("set_instance_sleep failed (batch lease)");
        sleepers.push(instance_id);
    }

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stranded = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    // Counted because the reader swallows read errors to stay out of the
    // claim's way. Without this, a run where every read failed would leave
    // `stranded` empty and pass the headline assertion having observed nothing.
    let samples = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // Holds the claim back until the reader has completed a full pass.
    //
    // `tokio::spawn` only queues a task, and an in-memory backend's `await`
    // points resolve inline without ever yielding to the executor — so on a
    // busy machine the whole claim can finish before the reader is scheduled
    // even once. Sequencing it here makes the observer's participation a fact
    // rather than a hope; without this the sample assertion below is itself a
    // race, and fails on whichever runner happens to schedule least eagerly.
    let observing = std::sync::Arc::new(tokio::sync::Barrier::new(2));

    let reader = {
        let backend = std::sync::Arc::clone(&backend);
        let stop = std::sync::Arc::clone(&stop);
        let stranded = std::sync::Arc::clone(&stranded);
        let samples = std::sync::Arc::clone(&samples);
        let observing = std::sync::Arc::clone(&observing);
        let watched = sleepers.clone();
        tokio::spawn(async move {
            // One sighting already fails the assertion; the rest are only
            // there to make the message concrete. Stop early rather than
            // keep sampling a backend that has already been caught.
            const ENOUGH: usize = 16;
            // Released after the first pass, never again.
            let mut gate = Some(observing);
            loop {
                for instance_id in &watched {
                    let Ok(Some(record)) = backend.get_instance(instance_id).await else {
                        continue;
                    };
                    samples.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if record.status == CoreInstanceStatus::Suspended
                        && record.sleep_until.is_none()
                    {
                        // Scoped so the guard is provably released before the
                        // await below; holding it across one makes this future
                        // non-`Send` and it cannot be spawned.
                        let seen_enough = {
                            let mut seen = stranded.lock().unwrap();
                            seen.push(instance_id.clone());
                            seen.len() >= ENOUGH
                        };
                        if seen_enough {
                            // Still release the claim, or it waits forever.
                            if let Some(gate) = gate.take() {
                                gate.wait().await;
                            }
                            return;
                        }
                    }
                }
                // Every row carries a past deadline at this point, so this
                // first pass can see no sighting — it only proves the reader
                // is live and reading before the claim begins.
                if let Some(gate) = gate.take() {
                    gate.wait().await;
                }
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                // Yield rather than sleep: the window this is hunting for is as
                // short as two adjacent statements.
                tokio::task::yield_now().await;
            }
        })
    };

    // Reached only once the reader has sampled every row at least once, so it
    // is provably watching before anything is claimed.
    //
    // Bounded so a reader that somehow never arrives fails this sequence
    // instead of hanging it: a test that never finishes is worse than one that
    // reports what went wrong. Expiring here leaves `samples` at zero, which
    // the assertion after the claim reports.
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        std::sync::Arc::clone(&observing).wait(),
    )
    .await;

    let ours: std::collections::HashSet<_> = sleepers.iter().cloned().collect();
    let mut claimed_by_us = Vec::new();
    for _ in 0..CLAIM_ROUNDS {
        // Sized to this sequence's own rows rather than a big round number: the
        // claim is global, so a larger limit would lease an unrelated backlog
        // 120s into the future on a shared store and delay real wakes.
        let batch = backend
            .claim_sleeping_instances_due(SLEEPERS as i64, Utc::now() + Duration::seconds(120))
            .await
            .expect("claim_sleeping_instances_due failed (batch lease)");
        if batch.is_empty() {
            break;
        }
        // Asserted over every record, not just ours: a returned row the caller
        // is told to launch must carry the lease it was claimed under, or a
        // failed launch has no deadline to fall back to.
        for record in &batch {
            assert!(
                record
                    .sleep_until
                    .is_some_and(|deadline| deadline > Utc::now()),
                "a claimed record must carry its lease deadline, but {} came back with {:?}",
                record.instance_id,
                record.sleep_until
            );
        }
        claimed_by_us.extend(
            batch
                .into_iter()
                .map(|r| r.instance_id)
                .filter(|id| ours.contains(id)),
        );
        if claimed_by_us.len() >= SLEEPERS {
            break;
        }
    }

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    reader.await.expect("lease reader task panicked");

    // An empty sighting list only means something if the reader could read at
    // all. This keeps "never observed the window" from reading as "the window
    // does not exist".
    assert!(
        samples.load(std::sync::atomic::Ordering::Relaxed) > 0,
        "the reader never sampled an instance successfully, so it proves \
         nothing about whether the claim leaves rows without a deadline"
    );

    // The assertion this sequence exists for.
    let sightings = stranded.lock().unwrap().clone();
    assert!(
        sightings.is_empty(),
        "a batch claim left {} row(s) `suspended` with no wake deadline, e.g. {:?}; \
         nothing sweeps that state, so a claimer that died here would strand them \
         permanently",
        sightings.len(),
        &sightings[..sightings.len().min(3)]
    );

    // Liveness. A backend that never claims would satisfy the assertion above
    // while waking nothing, and that is the other half of this method's
    // contract. But the claim is global with no tenant filter, so a rival test
    // sharing this store may legitimately take these rows first -- and losing
    // that race is not a defect. What must hold either way is that every row
    // ended up leased by *somebody*: still suspended, still carrying a future
    // deadline. Only a store where nothing was claimed at all fails here.
    let mut leased_by_someone = 0usize;
    for instance_id in &sleepers {
        let record = backend
            .get_instance(instance_id)
            .await
            .expect("get_instance failed (batch lease liveness)")
            .expect("sleeper should exist");
        if record.status == CoreInstanceStatus::Suspended
            && record.sleep_until.is_some_and(|d| d > Utc::now())
        {
            leased_by_someone += 1;
        }
    }
    assert!(
        !claimed_by_us.is_empty() || leased_by_someone > 0,
        "none of {SLEEPERS} due instances was claimed by this caller or leased \
         by any other in {CLAIM_ROUNDS} rounds: this claim never succeeds, so \
         nothing would ever wake"
    );

    for instance_id in &sleepers {
        backend
            .update_instance_status(instance_id, CoreInstanceStatus::Completed, None)
            .await
            .expect("update_instance_status failed (batch lease cleanup)");
    }
    backend
        .delete_instances_batch(&sleepers)
        .await
        .expect("delete_instances_batch failed (batch lease cleanup)");
}
