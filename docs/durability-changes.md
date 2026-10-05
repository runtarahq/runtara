# Durability lifecycle: change plan

Status: implemented, 2026-10-05, all seven phases. End-to-end check:
`e2e/test_durability_lifecycle.sh`.

As built, where it differs from the plan below:

- **Lease owner.** The root lease owner is the runner registration (its
  container handle id). The scoped runner no longer claims a lease of its own.
  The run task owns the lease for the whole run and releases it last, after it
  has parked, because the park must still present it.
- **Unbound hosts.** A run with no supervisor lease claims one itself only when
  it promoted itself (an ungated launch) or is scoped. A gated launch whose
  supervisor bound no lease runs unowned.
- **Park release cause.** Migration 045 adds `parked BOOLEAN`, not a
  `released_by TEXT` column. The park sets it under the row lock, and every
  claim resets it.
- **Park API.** `ParkOutcome` carries no epoch, since the caller holds the
  token. Owned parking is a new `park_execution`. `park_instance_on_targets`
  stays the unowned host path.
- **Completion.** Owned completion (`CompleteInstanceParams::owned_by`) keeps
  the `bool` result. Its callers treat "already applied" and "superseded" the
  same way, so a typed outcome would carry nothing they use.
- **Park retry.** The run retries the park 5 times with backoff, then reports
  `RunExitReport::SuspendNotParked` to its monitor. The monitor applies
  `ExitKind::UnparkedSuspend` through `recover_or_fail_because(...,
  RecoveryCause::ParkFailed)`. Migration 046 adds `park_failed`.
- **Recovery wake (existing bug, fixed).** `mark_for_recovery` stamped
  `sleep_until` without `wake_reason`, and the launch queue refuses a wake
  launch without one. Every run recovered through `recover_or_fail` therefore
  stayed suspended. It now sets `wake_reason = 'recovery'`.
- **Writers without a lease.** A guest write that presents no lease (the
  instance protocol over HTTP, tests) is admitted only while no execution holds
  the root. Host-side writers keep their own unfenced methods (`insert_event`,
  `save_retry_attempt`, `save_checkpoint`, `complete_instance` without an
  owner). The run task's own post-exit writes (cancel and cleanup aborts)
  present the lease.
- **Lock order.** An owned fence on a write that does not touch the instance
  row takes `FOR KEY SHARE` on the instance row, then `FOR SHARE` on the lease
  row, in one statement. That is the same order every lifecycle writer uses, so
  the two cannot deadlock. A refusal is `CoreError::Superseded`, and it stops
  the guest.
- **Command acknowledgements.** A guest's acknowledgement of a pause or cancel
  command stays unfenced. It applies the command the operator issued, whichever
  execution acknowledges it.
- **Checkpoint lookup.** The emitter already reads through `get-checkpoint`, so
  no guest code changed. `handle_checkpoint_call` is the single transport
  adapter for the external empty-bytes form. `invocation_checkpoint` still
  accepts empty bytes for compatibility, beside the new
  `invocation_checkpoint_lookup`.
- **Lifecycle projection.**
  - `executionPhase` is on the executions API (`WorkflowInstanceDto`).
  - `waiting_in_process` comes from a process-local registry
    (`runtara_environment::in_process_waits`). The runtime host marks it for
    timer sleeps, `blocking-sleep` and in-process durable sleeps, through the
    new `RuntimeHost::in_process_wait` hook.
  - Migration 047 adds the `paused` event. A guest-emitted breakpoint pause
    still records `suspended`.
  - The direct compiler rejects a non-durable Delay, so today the only
    in-process waits are non-durable Agent retry backoffs.

Verified 2026-10-05:

- **New tests.** Durability conformance (`conformance/durability.rs`) runs
  against both backends. Each new test was also run against the old behaviour
  to confirm it fails:
  - the racing checkpoint handler (`stale_root_write_waiting_on_a_lock_*`),
    plus `tests/conformance.rs` in store-postgres;
  - the runtime host tests (superseded host, empty-bytes call, in-process
    wait);
  - promotion and the root lease (`launch_queue_test`, `embedded_runner_test`);
  - park retry and recovery (`agent_suspension_runner_test`);
  - a suspended exit with a failed cleanup (component-host).
- **Existing suites.** All pass:
  - core, store-postgres, environment (database and scoped);
  - component-host with its integration features;
  - `runtara-workflows`, including `direct_wasm_execute`;
  - the runtara-server database and Valkey suites;
  - the frontend `npm test`, lint and build;
  - `cargo clippy --workspace --all-targets` with every CI gate feature except
    `embed-ui`, which needs a built frontend.
- **End to end.** `e2e/test_durability_lifecycle.sh` passes against a live
  server: owned park and a relaunch under epoch 2, the paused event and phase,
  five injected park failures recovered as `park_failed` then completed, and
  `waiting_in_process` during a retry backoff.

This plan makes checkpointing, suspension, and recovery follow one ownership
contract. It is a reliability change, not a rewrite. Checkpoint identities,
replay, continuation payloads, the WIT surface and workflow-author APIs stay as
they are.

## Contract

Every phase below serves these invariants:

1. **One owner.** Exactly one execution may change a root instance's durable
   state at a time. Once a replacement execution owns the instance, writes from
   an older execution are rejected by the store, not by runner discipline.
2. **First result wins.** A result checkpoint, once committed, is never
   replaced, whoever writes second.
3. **Suspension is a committed transition.** An execution that stopped to
   suspend is either durably parked or handed to recovery. A failed park never
   becomes a terminal failure while the instance could be resumed.
4. **Retries are safe.** Repeating a park, completion, or checkpoint with the
   same owner returns the original outcome instead of a new rejection.

## Where we are

Verified against `main` at `f6bc8123`.

| Area | Today | Gap |
|---|---|---|
| Ownership tokens | `invocation_root_leases` (owner, epoch, active) and `AttemptFence` exist in `runtara-store-postgres/src/invocations.rs`. A status trigger revokes the root lease whenever an instance leaves `running` (`030_instance_input_requests.sql`). | Only the scoped runner claims a root lease (`runner/embedded/scoped.rs`). Ordinary root runs carry no token. |
| Root writes | Park, `complete_instance(if_running)` and continuation puts check `status = 'running'`. `save_checkpoint`, `update_instance_checkpoint`, `insert_event`, `save_retry_attempt` and the durable sleep write check nothing. | Once B's `mark_running` sets the row back to `running`, a stale runner A can checkpoint, park, complete, or append events. Only in-process stops (cancel flag, epoch interrupt) prevent this. |
| Result checkpoints | The root path in `instance_handlers/checkpoint.rs` reads, then writes `ON CONFLICT DO UPDATE` (`dialect/postgres.rs`). | Two concurrent writers both miss the read and the last one wins. The fenced path (`invocations.rs`) already does `DO NOTHING` plus a re-read. |
| Park | `park_invoke_suspend` (`runner/embedded.rs`) logs `Err` and `Rejected` at `warn` and returns `()`. `lifecycle::park` rejects any status but `Running`. | A database error during park leaves the row `running`. The monitor then records a crash and marks the run Failed. A lost acknowledgement takes the error branch, which skips `wake_if_signal_already_arrived`. A retried park can't tell its own committed park from a pause or cancel. |
| Checkpoint lookup | Empty state bytes mean "read" in both the root and fenced paths. An explicit `get-checkpoint` and `handle_get_checkpoint` already exist. | The empty-bytes overload remains the internal contract. |
| Suspension boundary | `RootExecutionCoordinator` reaps descendants before a `Suspended` exit is published. The emitter keeps suspending work out of parallel windows. | Order: continuation write → guest exit → reap → park. A crash between the continuation write and the park leaves the row `running`, and nothing tests it. |
| Status model | `Suspended` covers both waiting and paused. Paused means `Suspended` with no suspension reason. | APIs and events can't distinguish "waiting in process", "durably suspended" and "paused". |

## Order

| # | Phase | Priority | Depends on | Why this position |
|---|---|---|---|---|
| 1 | First-wins root result checkpoints | P0 | none | Small and standalone. It closes a real overwrite race and also makes phase 5 cheaper. |
| 2 | Root execution lease for every run | P0 | none | Supplies the token that phases 3–5 check. It changes no write semantics yet. |
| 3 | Fenced lifecycle transitions with typed outcomes | P0 | 2 | Park and completion are the highest-impact stale writes, and an idempotent park needs the token. |
| 4 | Park recovery | P0 | 3 | Turns a failed park from a terminal failure into recovery. It needs phase 3's typed outcome to tell "retry" from "superseded". |
| 5 | Fence the remaining root writes | P1 | 2 (3 for the shared helpers) | Checkpoints, continuations, events, sleep and retry records. Mechanical once the helper from phase 3 exists. |
| 6 | Explicit checkpoint lookup internally | P2 | 1 | Clarity. No behaviour change. |
| 7 | Lifecycle projection | P2 | none | Observability. No storage change. |

Phases 1, 6 and 7 can go in parallel with anything. Phases 2→3→4 are one
sequence, and should land as separate PRs in that order.

---

## Phase 1: First-wins root result checkpoints

**Change**

- Replace the root `sql_save_checkpoint` upsert with
  `INSERT ... ON CONFLICT (instance_id, checkpoint_id) DO NOTHING`, followed by
  re-reading the row on conflict. This matches `invocations.rs`.
- `handle_checkpoint` returns the stored bytes as `found: true` when the
  insert lost. The guest already resumes from `found` bytes, so it adopts the
  winner.
- Move `update_instance_checkpoint` into the same transaction as the insert, so
  the instance's current checkpoint pointer can't drift from the stored row.
- Leave durable sleep (`handle_sleep`) and retry records as replace-semantics
  writes. They are continuation-like by design; phase 5 fences them.

**Files:** `runtara-store-postgres/src/dialect/postgres.rs`,
`ops_common/ops/checkpoints.rs`, `runtara-core/src/instance_handlers/checkpoint.rs`,
`runtara-core/src/persistence/memory.rs` (parity).

**Tests**

- A conformance case where two concurrent `save_checkpoint` calls on one
  address both return the first committed bytes.
- A handler test where a write after an existing row returns `found: true` with
  the original bytes.

**Done when** no code path can replace an existing `(instance_id, checkpoint_id)`
row in `checkpoints`.

## Phase 2: Root execution lease for every run

**Change**

- Claim the root lease inside the `mark_running` transaction
  (`launch_queue.rs`), in the same transaction that promotes the instance to
  `running`. One upsert into `invocation_root_leases` sets owner to the
  launch's `lease_owner`, `epoch` to `previous + 1` and `active` to true.
  - It needs no previous-lease check. `mark_running` only promotes from
    `pending` or `suspended`, and the status trigger has already revoked any
    lease held while the row was last `running`.
  - Decided: claim in the transaction, not straight after it.
    - `mark_running` already writes the store's `instances` table, so this
      adds no new coupling.
    - Claiming afterwards would create a new failure, a run that is `running`
      with no owner.
    - It adds no extra round trips per launch.
- `mark_running` returns the `InvocationLease`, and the launch handoff carries
  it to the runner.
- Pass the token into `PersistenceRuntimeHost` at construction. Replace the
  `input_lease: OnceLock` with a single `lease: InvocationLease` that managed
  inputs also read.
- The scoped runner takes the handed-off token instead of calling
  `RootLease::claim`, which would now be rejected because the lease is already
  active. `RootLease` keeps only its exact-token `release`, and moves from
  `runner/embedded/scoped/` to `runner/embedded/` so both paths share it.
- Release the lease on run-task exit, as the scoped runner already does.
  `LeaseMismatch` and `UnknownRoot` on release remain benign.
- Recovery needs no change. `mark_for_recovery` moves the row out of `running`,
  the trigger revokes the old lease, and the next `mark_running` claims
  `epoch + 1`.

**Files:** `runtara-environment/src/launch_queue.rs`, `runner/embedded.rs`,
`runner/embedded/scoped.rs`, `runner/embedded/scoped/lease.rs` (move),
`runtime_host.rs`, `runtime_host/inputs.rs`.

**Tests**

- An embedded runner test where an ordinary root run holds an active lease
  while running and the lease is inactive after suspension or completion.
- Recovery after a lease-expiry relaunch produces `epoch + 1`.
- A launch-queue test where `mark_running` and the lease claim commit or roll
  back together: no `running` row without an active lease.

**Done when** every running root instance has exactly one active lease row, and
the host can reach that token.

## Phase 3: Fenced lifecycle transitions with typed outcomes

**Change**

Add an owner token to the guest-driven lifecycle writes, and return a typed
outcome instead of `Decision`:

```rust
pub enum ParkOutcome {
    /// This call committed the park.
    Parked { epoch: i64 },
    /// This owner already committed this park; the retry is a no-op.
    AlreadyParked { epoch: i64 },
    /// Another owner, a pause, a cancel, or a terminal transition won.
    Superseded,
}
```

- `park_instance_on_targets` takes `&InvocationLease`. Within the existing
  `FOR UPDATE` transaction (`store-postgres/src/lifecycle.rs`), check the lease
  before `lifecycle::park`:
  - The lease is active and matches: apply the park and return `Parked`.
  - The lease matches but is inactive, the status is `Suspended`, and the
    lease's release cause is `park`: return `AlreadyParked`.
  - Anything else: return `Superseded`.
  - Storage errors stay `Err`.
- Record why a lease was released. Add `released_by TEXT NULL` to
  `invocation_root_leases` as a forward migration. Park sets it to `park`, and
  the status trigger sets `status_change` for every other exit from `running`.
  This is the only schema change in the plan.
- Guest-driven `Completed`, `Failed` and `Suspended` events
  (`instance_handlers/event.rs` → `complete_instance(if_running)`) also take the
  token. They gain the same `Applied` / `AlreadyApplied` / `Superseded`
  distinction. `CompleteInstanceParams` gets `.with_owner(&lease)`.
- Monitor and recovery writes (`ObservedExit::apply`, `recover_or_fail`) keep
  their current authority. They are fenced by launch id, attempt and registry
  handle, and they act on behalf of the host, not the guest.
- `lifecycle::park` stays a pure status policy. Ownership is checked in the
  store, next to the row lock, never in the runner.

**Files:** `runtara-core/src/lifecycle.rs` (outcome types),
`runtara-core/src/persistence/mod.rs` (trait signatures),
`runtara-core/src/persistence/memory.rs`, `runtara-store-postgres/src/backend.rs`,
`runtara-store-postgres/src/lifecycle.rs`, `runtara-core/src/instance_handlers/event.rs`,
a new `runtara-store-postgres/migrations/postgresql/045_root_lease_release_cause.sql`,
and `.sqlx` metadata if any checked query changes.

**Tests (conformance, run against memory and Postgres)**

- **Lost acknowledgement.** Park commits, then the same owner retries the
  same park. The retry returns `AlreadyParked`, and no second wake or event is
  written.
- **Stale park.** Owner A's lease is revoked, the run is relaunched as B, and
  A parks. A gets `Superseded`, and B's row is unchanged.
- **Stale completion.** Same setup, with A sending `Completed`. A is rejected
  and B's run continues.
- **Pause or cancel during park.** A pending pause is applied, then the owner
  parks. The park returns `Superseded`.

**Done when** no guest-driven lifecycle write accepts a call without a token,
and the policy matrix in `conformance.rs` covers all three outcomes.

## Phase 4: Park recovery

**Change**

- `park_invoke_suspend` returns `Result<ParkOutcome, CoreError>` instead of
  `()`.
- On `Err`, retry with bounded backoff, reusing the shape of
  `settle_with_retry`. The same token makes the retries safe: phase 3 turns a
  commit with a lost acknowledgement into `AlreadyParked`.
- On `Parked` or `AlreadyParked`, run `wake_if_signal_already_arrived` for
  signal parks. This fixes the lost-acknowledgement path, which currently skips
  it.
- On `Superseded`, do nothing. Another transition owns the instance.
- If retries run out, the run task reports a new exit kind,
  `ExitKind::UnparkedSuspend`, through the observed-exit record. Its
  `ObservedExit::apply` routes through `recover_or_fail` rather than writing
  Failed. The instance becomes `Suspended` with a recovery wake and replays from
  its checkpoints. The `RUNTARA_MAX_AUTO_RESTARTS` crash-loop cap still bounds a
  database that stays broken. The termination reason is `park_failed`, kept
  distinct from `crashed` and `shutdown_requested`.
- The continuation was already stored before the guest exited, so the replay
  resumes the suspended agent operation instead of re-running it.

**Files:** `runtara-environment/src/runner/embedded.rs`, `observed_exit.rs`,
`handlers.rs` (monitor wiring), `recovery.rs`.

**Tests**

- **Park failure.** Inject a failing `park_instance_on_targets` (a test
  persistence wrapper). The instance ends `Suspended` with reason `park_failed`
  and a recovery wake, not `Failed`. The wake relaunches it and it completes.
- **Continuation saved, park never committed.** Kill the runner between the
  agent's continuation write and the park. Recovery resumes the agent
  operation from its continuation exactly once.
- **Lost acknowledgement plus an early signal.** The signal arrives before the
  park, and the park's acknowledgement is lost. The run still wakes.
- **A database that stays down.** After the cap is reached, the instance fails
  with the crash-loop reason instead of looping forever.

**Done when** no path logs a park failure and drops it, and a transient storage
error during park never fails a resumable run.

## Phase 5: Fence the remaining root writes

**Change.** Give the token to every remaining guest-driven write and check it
in the same statement or transaction as the write:

| Write | Today | After |
|---|---|---|
| `save_checkpoint` + `update_instance_checkpoint` | nothing | active lease check plus phase 1's first-wins insert |
| Continuation put (`continuations.rs`) | `status = 'running' FOR SHARE` | join on the active lease (owner and epoch) instead of status |
| `insert_event` (guest-originated) | nothing | active lease check; host and monitor events stay unfenced |
| Durable sleep (`handle_sleep`) | read, then unconditional write | one fenced write |
| `save_retry_attempt` | nothing | active lease check |
| Guest-triggered `schedule_wake` | terminal-only guard | active lease check |

- Build one helper, `with_root_owner(tx, &lease)`, from the existing
  `check_lease` in `invocations.rs`. Each fenced write calls it instead of
  hand-writing the predicate.
- `ensure_instance_running` read-then-write checks in `checkpoint.rs` go away,
  because the fenced statement makes them redundant.
- Rejections surface as a new `CoreError::Superseded`. The runtime host then
  sets the cancel flag and stops the guest, exactly as it already does on
  lease-watch loss.

**Files:** `runtara-core/src/persistence/mod.rs`, `memory.rs`, `conformance/*`,
`runtara-store-postgres/src/{backend,continuations,invocations}.rs`,
`ops_common/ops/*`, `runtara-core/src/instance_handlers/{checkpoint,event}.rs`,
`runtara-environment/src/runtime_host.rs`.

**Tests**

- **Stale writer matrix.** In one conformance case, owner A is revoked and B
  claims the instance. Each write from A in the table above is rejected, and
  B's state is untouched.
- **Write blocked on a lock.** A write from A waits on a row lock while B
  claims. A sees B's committed lease and is rejected. This mirrors
  `invocation_write_waiting_on_a_database_lock_observes_committed_fence`.

**Done when** the only unfenced writes to a root instance are host-side
(monitor, recovery, API commands), and each of those is documented as such.

## Phase 6: Explicit checkpoint lookup internally

**Change**

- Split the core persistence and handler API into `lookup_checkpoint` (read
  only) and `record_checkpoint` (first-wins write, never a probe). Remove the
  `request.state.is_empty()` branch from the internal handler.
- Keep the external WIT contract unchanged. The `checkpoint` call with empty
  state still means "read", and the host adapter translates it to
  `lookup_checkpoint`. The stdlib WIT note stays accurate.
- Do the same for `InvocationCheckpoint`: the fenced read and write become two
  calls internally.
- Move stdlib and emitter call sites that send empty bytes to `get-checkpoint`
  where they compile, then mark the empty-bytes form as compatibility-only in
  the WIT doc comment.

**Tests.** The existing checkpoint tests should pass unchanged. Add one
adapter test showing that empty bytes from the guest still read and never
write.

## Phase 7: Lifecycle projection

**Change.** Derive a read-only `execution_phase` from the existing columns. No
schema change.

| Phase | Derived from |
|---|---|
| `running` | `status = running` and no in-process wait |
| `waiting_in_process` | `status = running` with an active in-process sleep or wait (already tracked by the runtime host) |
| `suspended` | `status = suspended` and `suspension_reason IS NOT NULL` |
| `paused` | `status = suspended` and `suspension_reason IS NULL` |

- Expose `execution_phase` on instance APIs and the Runs view, alongside the
  existing `status`.
- Regenerate the frontend client with `generate-api-runtime-offline`.
- Emit a distinct `paused` event. Today a pause and a suspension both emit
  `Suspended`.

**Tests.** One API test per phase, plus a frontend render check in the Runs
view.

---

## Failure scenarios coverage

| Scenario | Existing coverage | Added in |
|---|---|---|
| Lost acknowledgement after a committed park | none | phase 3 (lost-acknowledgement conformance), phase 4 (signal case) |
| Event arriving during suspension | `park_self_wakes_when_the_signal_beat_the_suspend_write`, `park_and_accept`, `instance_waits_race.rs` | none needed |
| Runner death after the continuation, before the park | none (only the clean path in `agent_suspension_runner_test.rs`) | phase 4 |
| Stale execution writes after relaunch | fenced child writes only (`lease_takeover`, `child_write_rejections`) | phases 3 and 5 |
| Cleanup failure with outstanding child work | `cleanup_failure_overrides_successful_root_result` and terminal-publication tests | add one case combining a `Suspended` exit with a cleanup failure (with phase 4) |
| Pause or cancel racing a wake | `pause_invalidates_claimed_and_queued_wakes_without_failing_the_root`, `parked_cancellation_and_launch_start_are_serialized`, `terminal_cancel_races` | phase 3 adds pause or cancel during park |

## Verification per phase

- `cargo test -p runtara-core`, and `cargo test -p runtara-store-postgres` and
  `-p runtara-environment` with their CI features against isolated test
  databases.
- `cargo clippy --workspace --features $GATE_FEATURES -- -D warnings`, because
  the trait signatures change.
- From phase 3: `cargo sqlx prepare` if checked queries change.
- From phase 4: an end-to-end run on an isolated server. Suspend a workflow on
  a signal, inject a park failure, check that it recovers, then deliver the
  signal and check that it completes.

## Non-goals

- No change to checkpoint identities, replay, continuation payloads, the WIT
  surface (beyond doc comments), or the DSL.
- No new instance status values. Phase 7 is a projection.
- No change to monitor, recovery or API-command authority. They are already
  fenced by launch identity.
