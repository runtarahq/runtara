# Performance Changes Review

Review range: `origin/main...feature/performance-improvements`

## Findings

### 1. Critical: Claimed sleepers can be permanently stranded after a process interruption

`crates/runtara-core/src/persistence/common/ops/sleep.rs:163-175` clears
`sleep_until` for the entire claimed batch.
`crates/runtara-environment/src/wake_scheduler.rs:237-274` launches the
instances only after that transaction commits.

If the process crashes, a task panics, or a deployment interrupts the scheduler
between those operations, instances remain `suspended` with
`sleep_until = NULL`. No startup recovery path discovers that state. A default
batch can strand 200 durable executions.

Claims need durable ownership and expiry, such as `wake_claimed_at` and
`wake_claim_owner`, with expired claims becoming eligible again.

### 2. High: Concurrent wake processing races with graceful drain

The scheduler checks drain once at
`crates/runtara-environment/src/wake_scheduler.rs:225-232`, then claims and
queues the full batch at `wake_scheduler.rs:237-274`. Drain sets the flag and
immediately snapshots the registry at
`crates/runtara-environment/src/runtime.rs:511-526`.

A batch that passed the initial check can launch after the snapshot. Those
instances receive no shutdown signal, are absent from the straggler list, and
can survive into runtime teardown.

Quiesce and join the wake scheduler before taking the drain snapshot. Also
recheck drain after acquiring each launch permit and release unlaunched claims.

### 3. High: Multiple trigger workers break `single_instance` enforcement

`crates/runtara-server/src/server.rs:1399-1425` starts several workers. Each
worker constructs its own `ExecutionEngine` at
`crates/runtara-server/src/workers/trigger_worker.rs:160-169`, and every engine
creates an independent `starting_workflows` set at
`crates/runtara-server/src/workers/execution_engine.rs:341-349`.

Two workers can simultaneously pass `has_running_instance` at
`trigger_worker.rs:517-539`, reserve only in their private sets, and launch two
different instance IDs.

Use an atomic shared reservation acquired before the check. A database lock or
uniqueness constraint is preferable because an in-memory lock still fails
across server replicas.

### 4. High: The concurrency gate admits an unbounded burst above its configured limit

`crates/runtara-server/src/workers/execution_engine.rs:361-370` caches only the
observed runtime count for 500 ms. Accepted requests do not increment or reserve
against that count before publishing at `execution_engine.rs:592-612`.

With a cap of one and a cached count of zero, every request arriving during that
interval is accepted. The new parallel trigger workers then start them
concurrently. The comment claiming overshoot is bounded to one TTL's worth still
permits hundreds of executions and violates entitlement limits.

Admission needs atomic reservations that are released on publish or start
failure and reconciled against runtime state.

### 5. High: Existing compiled workflows will not receive the new store-freeing sleep default

The lowering now depends on `RUNTARA_DIRECT_STORE_FREEING_SLEEP` at
`crates/runtara-workflows/src/direct_wasm/compile.rs:868-915`, but the selected
mode is absent from artifact and cache identity. Image reuse checks only source
checksum, template major, and compiler mode at
`crates/runtara-server/src/api/services/compilation.rs:75-85`.

Existing workflows with unchanged definitions remain on blocking sleep after
deployment. Similarly, setting the rollback variable to `false` can keep
serving an existing store-freeing artifact unless every workflow is forcibly
rebuilt.

Include this lowering mode or a compiler revision in image metadata and cache
matching, or bump the template major and force recompilation.

### 6. Medium: A broken full wake batch retries in a database and logging hot loop

Failed wakes are restored with `sleep_until = now` at
`crates/runtara-environment/src/wake_scheduler.rs:341-347`. The scheduler
returns the original claim count and immediately polls again for a full batch at
`wake_scheduler.rs:204-208`.

If 200 sleepers all reference missing images or encounter a persistent launch
failure, the same 200 rows are reclaimed continuously with no delay.

Restore failed claims with bounded exponential backoff and base immediate
polling on successful progress, not the number originally claimed.

### 7. Medium: `RUNTARA_DB_CLEANUP_BATCH_SIZE=0` causes an infinite cleanup loop

Environment parsing accepts zero at
`crates/runtara-environment/src/db_cleanup_worker.rs:80-105`. The debug sweep
then repeatedly deletes zero rows because `deleted < batch_size` becomes
`0 < 0`, which is false at `db_cleanup_worker.rs:242-253`.

This spins on PostgreSQL and prevents runtime shutdown from joining the worker.
A zero poll interval similarly causes continuous cleanup cycles.

Reject non-positive values and use checked duration arithmetic.

### 8. Medium: Heartbeat cleanup can still fail a replacement container

The stale-container query snapshots `container_id` at
`crates/runtara-environment/src/heartbeat_monitor.rs:220-281`, but
`fail_stale_instance` later updates and cleans up by instance ID without
verifying ownership at `heartbeat_monitor.rs:289-350`.

If a sleeper relaunches between selection and processing, the old stale record
can mark the new run failed and delete its fresh registry row.

Recheck that the registry still contains the selected `container_id`. Make both
the state transition and cleanup conditional on that generation.

### 9. Medium: The new E2E does not validate its advertised multi-batch behavior

`e2e/test_wake_drain_throughput.sh:42` creates 40 instances against a new batch
size of 200. The entire backlog fits in one poll, so restoring the five-second
delay between full batches would still pass.

The launch loop at `e2e/test_wake_drain_throughput.sh:194-202` also discards
responses and uses `curl` without `--fail`, counting rejected HTTP requests as
launched.

Run more than one explicitly configured batch, record every returned instance
ID, and assert consecutive full claims occur without the idle interval.

### 10. Low: The claim-release regression test can pass before the scheduler runs

`crates/runtara-environment/tests/wake_scheduler_test.rs:677-688` immediately
considers any suspended instance with a deadline restored, but that is its
initial state. The test can pass even if claim release is removed.

First observe the claim state or synchronize on a launch attempt, then assert
the deadline is restored.

## Scope Note

The branch also contains `fd74ead5`, which removes Claude skills and adds
assistant configuration. It is unrelated to runtime performance and should
ideally be split into a separate pull request.

## Verification

- `cargo test -p runtara-core --lib`: 72 passed.
- `cargo test -p runtara-environment --lib`: 99 passed.
- `cargo test -p runtara-server --lib workers::execution_engine`: 10 passed.
- `cargo test -p runtara-server --lib workers::trigger_worker`: 9 passed.
- Targeted workflow compiler tests and shell syntax checks passed.
- Database-backed integration tests were compiled but not executed.
- `git diff --check origin/main...feature/performance-improvements` passed.
