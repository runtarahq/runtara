# Throughput improvements

Status: implemented and verified locally. Changes are not pushed.

## Target invariants

- A start may queue briefly, but the queue is durable, bounded, and cancellable.
- No request, trigger worker, or wake worker waits for a runner permit.
- A suspended approval consumes no runner capacity, admission capacity, or
  `single_instance` lease. The system must be able to host millions of parked
  approvals.
- Tenant environments are isolated. Cross-tenant fairness is not required.
- Direct workflow artifacts use the `lifecycle.invoke` ABI. Legacy
  `wasi:cli/run` workflow artifacts are unsupported.

## P0: eliminate system-blocking paths

| ID | Gap | Implemented fix | Expected outcome | Status |
| --- | --- | --- | --- | --- |
| P0.1 | A full runner makes start, wake, and trigger work wait indefinitely for a permit | Added durable `instance_launches`; starts, resumes, and wakes enqueue and return. A bounded dispatcher is the sole runner caller and uses non-blocking capacity acquisition. | Trigger and wake workers remain available while the runner is full. No unbounded in-memory semaphore waiters exist. | Landed locally |
| P0.2 | A queued launch has no deadline or cancellation path | Launches carry a generation, deadline, lease, availability, retry count, and error. Expiry becomes `launch_queue_timeout`; cancellation is terminal and releases admission. | Valid but starved work cannot occupy capacity forever. A generic `pending` age sweep is not needed. | Landed locally |
| P0.3 | Admission can be bypassed or lost while trigger events are queued | All intake paths use durable admission plus an idempotent outbox/request record, then a relay publishes it to the worker stream. | The queue remains bounded even during publisher failures or a worker outage. | Landed locally |
| P0.4 | A run can start before its registry, monitor, or state transition is reliably installed | A generation-owned, start-gated handoff installs durable state and monitoring before guest instantiation. A failed registry or state transition keeps the gate closed. | Every active run has recoverable ownership and a deadline before it executes. | Landed locally |
| P0.5 | Cleanup for an old attempt can affect an immediate resume | `launch_id`/attempt fencing now follows the runner handle, task map, registry, monitor, stop path, and durable state writes. | An old attempt cannot unmonitor, stop, or delete a newer attempt. | Landed locally |
| P0.6 | Legacy `wasi:cli/run` workflows retain runner slots while waiting | Generated direct workflows require `lifecycle.invoke`; legacy `wasi:cli/run` direct artifacts fail before runner acquisition. Generic agent components retain their existing ABI. | Current-ABI approvals still park and free their runner slot; legacy workflow artifacts cannot block the pool. | Landed locally |
| P0.7 | A workflow published as an agent, or a non-durable wait, can sleep inside its parent runner | Workflow-agent graphs with wait-like operations and non-durable waits are rejected; supported top-level scheduled waits suspend. | No hidden runner-held wait path remains. | Landed locally |

## P1: bound execution and make it operationally safe

| ID | Gap | Implemented fix | Expected outcome | Status |
| --- | --- | --- | --- | --- |
| P1.1 | Retry and rate-limit backoff sleep inside a runner | Retry state and an absolute wake time are durable; the instance suspends and resumes to consume the retry exactly once. | A burst of rate limits parks work instead of consuming all runner slots. | Landed locally |
| P1.2 | Agent and embedded-workflow step timeouts are advisory | Unsupported per-step timeouts are rejected rather than accepted as advisory configuration. | A timeout is either enforced or rejected rather than silently treated as a hint. | Landed locally |
| P1.3 | HTTP can hang after headers, and direct calls have no universal default | One absolute policy deadline covers connection, headers, and body collection; response size and caller timeout are capped. | Slow or endless HTTP bodies cannot hold a runner indefinitely. | Landed locally |
| P1.4 | Execution timeout defaults and validation disagree | A typed execution policy is shared by validation and every launch/resume path, with server-side bounds. | No API request can create an effectively unbounded active run. | Landed locally |
| P1.5 | Preparation, database, disk, or compile work can outlive the external monitor | Added durable `preparing` leases, bounded dispatcher/preparation pools, and a short-lived trusted child for artifact read, hash, and Wasmtime precompile. Cancellation/expiry kills and reaps that child; parent linking is bounded in-memory work before the gated handoff. | No disk or compile task can consume a run slot or indefinitely occupy preparation capacity. | Landed locally |
| P1.6 | `single_instance` can race while a launch is being created | A durable workflow-scoped active lease covers `queued`, `preparing`, `leased`, `starting`, and `running`; it is released atomically at suspension and terminal outcomes. | Duplicate concurrent launches are prevented without limiting parked approvals. | Landed locally |
| P1.7 | Pipeline reports a symptom but cannot identify or act on it | The pipeline reports durable stages, preparation/child-reaping occupancy, oldest age, capacity pressure, and workflow attribution. | Operators can identify and resolve the actual blocked stage. | Landed locally |

## Detailed implementation plans

### P0.1 — Durable launch dispatcher

**Approach.** Add an `instance_launches` table with a UUID `launch_id`,
`instance_id`, image identity, launch kind (`start`, `resume`, or `wake`),
state, scheduling time, attempt count, lease owner/expiry, deadline, and last
error. A partial unique index must allow at most one non-terminal launch for an
instance. The initial-start transaction creates the instance, image binding,
and launch record together. A resumed or woken instance creates a new launch
generation while retaining its durable checkpoint.

Add an Environment-owned dispatcher which claims a small batch with `FOR UPDATE
SKIP LOCKED`, uses a non-blocking runner-capacity operation, and returns a busy
row to `queued` with a short, jittered retry time. Only that dispatcher may
call the runner. The existing start handler, resume handler, and wake scheduler
must enqueue and return; none may await `launch_detached`. A notification makes
the common path prompt, while a periodic scan provides recovery after a lost
notification or restart. Scheduling may prioritize launch kinds within one
tenant (for example, fresh start versus a wake storm), but it does not need
cross-tenant fairness because each tenant has its own Environment.

**Tests.**

- Hold every runner permit, submit more starts and wakes than the queue cap,
  and assert that trigger-worker and wake-worker permits are returned promptly.
- Release one permit at a time and prove that exactly one queued generation is
  launched each time.
- Restart between enqueue, claim, and dispatch; prove that an expired lease is
  recovered once and that no row is lost.
- Exercise duplicate start requests with the same idempotency key and assert
  that they return the same launch instead of creating a second row.

**Corner cases.** A wake and a manual resume can race for the same suspended
instance; the partial unique constraint must make one an idempotent result.
The dispatcher must not turn a suspended instance into `pending` until it owns
the launch handoff, otherwise a parked approval would accidentally count as
active. Draining must stop new claims but leave queued records durable for the
next process.

**Risks and mitigations.** Polling can add latency and a naive queue can create
database contention. Use indexed, small `SKIP LOCKED` batches, notification
plus polling, and bounded dispatcher concurrency. Do not replace the current
semaphore wait with one task per launch: that recreates the unbounded in-memory
queue that the current code deliberately avoids.

### P0.2 — Queue deadline and cancellation

**Approach.** Set an absolute `deadline_at` when a launch is enqueued. A
reaper owned by the dispatcher terminalizes only launch rows whose policy
deadline has passed, not arbitrary old `pending` instances. Cancellation uses
a single conditional transaction: it marks a `queued` or `leased` launch
cancelled, releases its admission reservation, and transitions the instance to
`cancelled`. Before acquiring capacity and immediately before opening the
guest's start gate, the dispatcher rechecks that its generation is still
launchable.

Once execution has begun, cancellation follows the normal generation-specific
runner stop path. Queue expiry is a failure with termination reason
`launch_queue_timeout`; an explicit user cancellation is `cancelled`. Both
operations are idempotent.

**Tests.**

- Use a controllable clock to expire a queued launch and assert its instance,
  launch row, and admission reservation reach consistent terminal states.
- Cancel before claim, during a lease, after a successful capacity acquisition,
  and immediately after guest start; assert the correct winner and no leaked
  permit.
- Repeat cancel and expiry requests concurrently to prove that only one
  terminal outcome is recorded.

**Corner cases.** A cancellation signal may arrive before the dispatcher has
ever observed the launch. A lease owner can die exactly at expiry. A retrying
caller may resend start after queue expiry. All three must resolve through the
same idempotency and generation checks rather than resurrecting the old row.
Use database time for deadline comparisons so host clock skew cannot create
different expiry decisions across dispatcher processes.

**Risks and mitigations.** A fixed queue deadline can reject useful work during
a temporary surge. Make it an Environment policy with visible queue-age
metrics, a conservative initial value, and a documented retry contract. Do not
silently extend deadlines on every retry: that turns a bounded queue back into
an indefinite one.

### P0.3 — Durable admission and outbox

**Approach.** Make the enqueue service the sole authority that accepts an
execution. In one database transaction it records a tenant-scoped admission
reservation, an idempotent execution/launch request, and an outbox event. A
relay publishes the outbox event to Valkey and marks the delivery separately.
Every producer, including cron, must use this service; patching only
`ExecutionEngine::check_concurrency_gate` is insufficient because cron can
publish directly today.

The reservation remains owned by the durable request until the launch parks as
`suspended`, expires, is cancelled, or its instance reaches a terminal result.
A stream redelivery finds the existing request instead of consuming another
reservation. Parked approvals retain neither a launch reservation nor an
admission reservation.

**Tests.**

- Inject a crash after the database commit but before stream publication, then
  prove the relay delivers the request after restart.
- Inject a crash after publication but before marking the outbox delivered and
  prove redelivery is idempotent.
- Submit concurrent HTTP, event, and cron requests at the admission limit and
  prove the configured bound is never exceeded.
- Verify terminal, cancelled, and queue-expired instances release exactly one
  reservation.

**Corner cases.** Valkey can be unavailable for longer than the queue deadline;
the outbox must expire the request rather than publishing obsolete work later.
Cron ticks and stream messages are at-least-once, so their event identity must
be stable. An image may become unavailable after admission; that is a terminal
launch failure, not a leaked reservation.

**Risks and mitigations.** An outbox adds database writes and retained rows.
Batch relay work, index undelivered rows, delete or archive acknowledged
terminal rows, and expose outbox age. Avoid claiming cross-system exactly-once;
instead make all consumers idempotent by request and launch ID.

### P0.4 — Generation-owned launch supervisor

**Approach.** Split runner handoff into preparation and a gated start. The
supervisor owns one launch generation: it verifies the queue lease, obtains
capacity, writes the generation-aware registry record, arms the watchdog, and
then opens the gate that lets the guest run. `mark_running` must be a checked,
generation-conditional transition; logging and continuing on its failure is
not acceptable. The runner receives a real deadline rather than `Duration::MAX`.

The supervisor becomes the single place that coordinates registry cleanup,
monitor completion, cancellation, and failure to prepare. The old external
monitor remains useful as a second observer, but cannot be the only timeout
owner.

**Tests.**

- Fault-inject every write between permit acquisition and guest start; assert
  that no guest executes when registry or state setup fails.
- Force the watchdog to expire during each launch phase and assert that only
  the owning generation is terminalized and its permit is released.
- Park immediately after start and verify that the active permit is released
  only after the durable suspension is committed.

**Corner cases.** A process can die after capacity acquisition but before the
gate opens; startup recovery must find the leased generation and either stop it
or requeue it safely. A component can complete before a monitor observes it.
The supervisor needs idempotent completion handling for both cases.

**Risks and mitigations.** This changes a central runner trait. Introduce a
small prepared/gated-launch interface alongside the existing implementation,
cover it with mock-runner tests, then remove the direct path after all launch
sources use the supervisor.

### P0.5 — Generation-safe task and registry cleanup

**Approach.** Key the embedded runner task map, run-slot tracking, container
registry, stderr/run directory, and runner handle by `launch_id`, not only
`instance_id`. Every cleanup operation must compare its generation before
deleting a record or transitioning an instance. The monitor and stop APIs
receive the same generation-bound handle. Each wake or resume creates a new
generation even when it retains the same durable instance ID. Persist an
`active_launch_id` fence (or equivalent) and require it for Core-visible
lifecycle writes: registry guards alone cannot stop a stale guest from writing
an old terminal result.

**Tests.**

- Stop a run and immediately resume it; let the old task finish last and prove
  it cannot remove the new task or registry row.
- Delay old monitor cleanup until after a new generation starts and prove that
  the new run remains monitored.
- Race heartbeat failure, timeout, manual stop, and wake against a resume.

**Corner cases.** A stale cleanup event can be delivered after a process
restart, and a handle may be unknown because its task already exited. These
must be harmless no-ops when their generation no longer matches. External APIs
may continue to expose `instance_id`; generation remains an internal safety
token rather than a breaking public identifier.

**Risks and mitigations.** Generation columns and conditional updates add
queries to hot paths. Keep them indexed, batch only observability reads, and
require generation conditions in repository helpers so new code cannot omit
them accidentally.

### P0.6 — Retire legacy direct-workflow `wasi:cli/run`

**Approach.** Remove the generated-workflow `CliRunHttp` option and the
`RUNTARA_DIRECT_WORKFLOW_ABI=cli-run` production switch. Bump the generated
workflow ABI/cache epoch, record the workflow ABI in image metadata, and use a
shared component classifier to verify the actual export rather than trusting
that metadata. Require `lifecycle.invoke` at both image registration and
launch. Before cutover, inventory image versions, rebuild everything that has
source, and mark unrebuildable legacy generated artifacts as unsupported with a
clear rebuild/re-publish action.

This applies only to generated direct workflows. Do not remove generic
`wasi:cli/run` support for unrelated agent components without an independent
compatibility decision.

**Tests.**

- Compiling with the removed environment setting fails loudly rather than
  quietly selecting the old ABI.
- A legacy generated image is rejected before a runner permit or registry row
  is acquired.
- A rebuilt invoke-ABI approval parks and releases its runner permit.
- Cache lookup cannot return a legacy artifact for a current compile request.

**Corner cases.** Some old images may lack ABI metadata. Treat unknown generated
workflow ABI as unsupported rather than guessing. A user can have a published
version that cannot recompile because a dependency disappeared; preserve the
definition and report the exact remediation, but never execute it through the
legacy path. A legacy artifact already running at cutover is allowed to drain
under its existing outer deadline; block new legacy starts rather than killing
an in-flight approval blindly.

**Risks and mitigations.** The cutover intentionally breaks old versions.
Mitigate with an inventory report, automated recompilation where safe, release
notes, and a staged environment rollout. Do not retain an emergency production
fallback: it would reintroduce the runner-blocking behavior this change removes.

### P0.7 — Reject blocking workflow-agent and non-durable wait shapes

**Approach.** Extend static workflow metadata with a transitive
`may_suspend_or_sleep` capability and a stable violation path. Validate a
workflow when it is published as an agent and again when a parent embeds/stages
it. Require an auditable non-suspending certification for staged workflow-agent
dependencies; old sidecars without it are republished or rejected. Reject
graphs containing waits, delays, retry/backoff, pause, breakpoint, or other
suspension-capable paths until the capability ABI can return `completed` or
`suspended(wakes)`. For normal top-level workflows, reject non-durable waiting
primitives rather than silently changing their replay semantics; compile every
supported positive durable scheduled wait into a persisted suspension,
including short delays.

**Tests.**

- Cover direct, nested, conditional, error-path, and parallel graphs whose
  transitive child contains a wait or retry.
- Verify dynamic delay expressions are rejected when non-durable and park when
  durable, regardless of their eventual duration.
- Verify a one-millisecond durable top-level delay parks, wakes exactly once,
  and does not re-run the delay after restart or an early manual resume.
- Assert that existing staged agent images are invalidated and cannot evade the
  new validation through a cache hit.

**Corner cases.** A graph can hide a wait in `onError`, `onWait`, or a nested
workflow. Metadata must describe every reachable subgraph, not only the happy
path. For an unknown third-party agent capability, default to "cannot suspend"
rather than trusting an absent declaration.

**Risks and mitigations.** This is an intentional compatibility break for some
valid-looking graphs. Return a stable error that identifies the offending step
and says to run it as a top-level workflow or remove the wait. A future
versioned suspending capability ABI can restore the feature without overloading
the current error/sentinel behavior.

### P1.1 — Durable retry and rate-limit backoff

**Approach.** Replace in-run `durable_sleep_checkpoint` retry paths with a first-class retry schedule: persist the failed attempt, retry identity, absolute wake time, cumulative rate-limit budget, and checkpoint before returning a top-level suspension. On wake, reload that state, verify the wake is due, consume the scheduled retry exactly once, and retry with the same idempotency context. Apply the lowering to Agent, EmbedWorkflow, sequential Split, and any parallel path that can currently sleep in the parent runner.

Until branch-level continuations exist, reject or explicitly degrade parallel split configurations whose retry semantics would require an independent branch timer; do not keep the parent runner alive as a compatibility shortcut.

**Tests.**

- Simulate `Retry-After`, confirm the instance parks and releases its runner slot, then wakes and retries once at the absolute deadline.
- Restart before and after recording the retry schedule and prove that an attempt is neither skipped nor duplicated.
- Cancel while parked, exhaust the rate-limit budget, and verify structured terminal results and admission cleanup.
- Exercise retry paths inside embedded workflows and split modes.

**Corner cases.** Wall-clock changes, a duplicate wake delivery, a retry due while an earlier attempt is still completing, and a zero or negative effective delay must all be deterministic. Persist absolute times and attempt numbers; never derive correctness from a local in-memory sleep. An external side effect may already have happened before a retryable response, so agent calls still need idempotency keys where their API supports them.

**Risks and mitigations.** Continuation state is easy to make replay-unsafe. Version and checksum the state, write it before suspension, and add crash-point tests around every transition. Enforce a maximum cumulative budget and retain the old inline path only long enough to reject unsafe graphs, not as a fallback.

### P1.2 — Enforce or reject step timeouts

**Approach.** In the short term, promote the current warning for Agent and EmbedWorkflow timeout fields to a validation error unless the target has a documented host-enforced deadline. In the long term, add a `StepDeadline` to the component-host invocation context, set a Wasmtime epoch/cancellation deadline before the capability call, and propagate a structured `STEP_TIMEOUT` through normal retry and error routing.

EmbedWorkflow is currently inlined, so an input `timeout_ms` alone cannot make it a hard child deadline. Do not claim otherwise. Either introduce an isolated child continuation/instance boundary or keep that timeout unsupported until such a boundary exists.

**Tests.**

- Run an agent that spins, one that waits on host I/O, and one that completes just before its deadline; verify the correct timeout and cleanup behavior.
- Verify a timed-out agent follows its configured retry/on-error path exactly once.
- Verify an embedded workflow timeout is rejected until child isolation lands.
- Race an outer execution timeout, explicit cancellation, and a step timeout and assert one stable termination reason.

**Corner cases.** The remaining outer deadline can be shorter than the step deadline; use the earlier deadline. A cancelled call can surface while a host future is unwinding. The implementation must wait for Wasmtime cancellation and resource cleanup rather than dropping a future and reusing a live store.

**Risks and mitigations.** True preemption changes semantics and can expose non-cancellable host code. Land validation first, then introduce an isolated host boundary with exhaustive cancellation tests. Document the versioned semantic change; do not leave a UI field that looks enforced when it is not.


### P1.3 — Bound HTTP for headers and bodies

**Approach.** Derive one absolute HTTP deadline from the smaller of the caller policy and remaining active execution deadline. Apply it to connection, headers, redirects, and body collection, rather than resetting a timeout for each phase. Add a maximum response-body size and cancel or drain the stream on timeout or size excess. Route generated workflow HTTP through this controlled path; the Object Model agent's direct WASI HTTP path must either adopt the same wrapper or be disallowed for workflow execution.

**Tests.**

- Simulate no headers, headers followed by a stalled body, a trickling body, endless chunked data, and an oversized declared or undeclared body.
- Verify cancellation releases the network resource and runner slot promptly.
- Assert that redirects and retries share the original absolute deadline rather than receiving a fresh full timeout.
- Verify a valid streaming use case fails clearly when it exceeds the configured body limit.

**Corner cases.** `Content-Length` may be absent or wrong, so enforce the limit while reading. A response can complete exactly at the deadline. A remote peer can keep a connection alive while making negligible progress. Tests must use deterministic clocks and local controlled servers for these boundaries.

**Risks and mitigations.** A global body cap can break legitimate downloads or streaming APIs. Make it explicitly configurable by approved connection or agent policy, require object storage for large payloads, and emit a precise `response_too_large` or timeout error instead of truncating data.

### P1.4 — One typed execution-timeout policy

**Approach.** Define a shared typed execution-policy object for active timeout, maximum active timeout, queue deadline, retry/rate-limit ceiling, and any response-body limit. Parse and validate it at workflow save/import, image readiness, synchronous launch, asynchronous launch, resume, wake, and direct Environment API boundaries. Use checked conversions and non-zero durations; never read JSON through signed narrowing casts. Persist the selected active deadline with the launch generation so a later configuration change cannot change an already accepted attempt unpredictably.

The active budget starts only after the launch supervisor opens the run gate. A durable suspension stops consuming runner capacity; a future policy decision can add a separate cumulative workflow-lifetime deadline without reusing the active budget incorrectly.

**Tests.**

- Submit raw API definitions with zero, negative/overflowing representations, and values above the cap; verify all entry points reject them consistently.
- Verify omitted values use the same default for sync, async, direct, resume, and wake paths.
- Start a run, change the system policy, and verify the stored generation keeps its original deadline while new launches use the new policy.
- Use a fake clock to prove an active guest is stopped at deadline while a workflow parked longer than that remains suspended and can later resume.

**Corner cases.** Existing stored definitions can contain values outside the new range. Inventory them before enforcement and choose an explicit migration: retain an already-running stored deadline, normalize a safe value, or reject a future launch with remediation. Handle clock skew and daylight-saving changes with UTC absolute deadlines only.

**Risks and mitigations.** Tightening the default can unexpectedly fail valid workflows. Preserve the current one-hour product maximum for the first rollout unless policy changes are separately approved, expose validation errors early in the editor/API, and report inventory counts before enforcement.


### P1.5 — Isolate preparation from runner permits

**Implemented approach.** A launch now enters durable `preparing` with an
attempt-scoped lease before any component or input work. The dispatcher and
runner each have bounded preparation pools; neither waits for a permit. An
ownership watcher begins before the Core/image reads, so cancellation, lease
recovery, or the preparation deadline drops in-flight option, input, and
precompile work before a bounded cleanup attempt. A stale attempt cannot
promote a token after another attempt has recovered the same launch.

Artifact filesystem reads, SHA-256 calculation, and Wasmtime component
precompilation run in a short-lived child of the trusted server executable,
not in the runner process. The private pipe protocol binds a fresh nonce, the
source digest, an engine fingerprint, and the serialized-artifact digest. It
caps source components at 64 MiB and serialized artifacts at 128 MiB. Generated
direct workflows also compare the source digest with immutable
`workflow.binaryChecksum` metadata; generic agent components keep their
existing ABI behavior. A deadline or cancellation kills and reaps the child;
both live and reaping children have a separate, host-memory-aware bounded pool.

The parent does not read the artifact, compile it, create a run directory, or
open stderr files on the durable preparation path. It verifies the child
response and performs the residual synchronous Wasmtime deserialize/link in
memory, then stores the opaque result only in the launch token—never in the
global component cache. This residual is size-capped, has no external I/O or
compiler work, and occurs before any live-run permit is acquired. The renewed
handoff lease bounds registry/Core work after promotion; an expired handoff is
recovered by its durable state and closed start gate rather than being
speculatively requeued.

All error cleanup, including a failed gate monitor, has a small independent
database-operation budget. If the database remains unavailable, the worker or
monitor exits and the exact durable lease/gate recovery path remains the
authority; it does not retain a preparation or runner slot.

**Tests.**

- Stall component loading, a database read, and a filesystem read separately;
  verify no runner permit is consumed and the preparation pool remains bounded.
- Cancel or recover a claim during option lookup and child compilation; verify
  that child work is killed/reaped before cleanup and cannot promote later.
- Exercise the lock-at-deadline race and prove that confirmation obtains the
  durable row lock before testing its deadline.
- Saturate preparation while runner capacity is available, then assert that the
  pipeline reports preparation saturation rather than falsely reporting runner
  starvation.
- Verify an artifact that changes or disappears between preparation and
  handoff is rejected safely.

**Corner cases.** A prepared artifact is not retained across launches: it dies
with the generation-owned token, so a cancellation or stale attempt cannot
create an unbounded native-artifact cache. The source/digest fence rejects an
artifact altered during preparation. Shutdown stops new claims while leaving
the durable row for lease recovery. A timeout after a runner handoff can leave
an unopened gated task or exact registry row briefly, but its gate closes at
the same durable lease deadline; it cannot invoke guest code or retain a
preparation permit.

**Risks and mitigations.** A separate pool adds queue stages and can shift,
rather than eliminate, a bottleneck. Its capacity, aged work, child reaping,
and precompile failures are visible in the pipeline. The only deliberately
non-killable portion is the bounded, pure in-memory parent deserialize/link
described above; isolating that too would require a separate managed linking
boundary and is not needed to protect runner capacity.

### P1.6 — Correct `single_instance` semantics for parked approvals

**Implemented approach.** The process-local start marker is replaced with a
durable workflow-scoped active-launch lease. The lease exists only while a
matching launch is `queued`, `preparing`, `leased`, `starting`, or `running`;
it is acquired atomically and released in the same transition that parks an
instance as `suspended`, or reaches a terminal outcome. A parked approval
therefore has no active lease, admission reservation, or runner slot.

The key must preserve the existing `single_instance` scope deliberately. Today the server checks running instances for the workflow, including work started by another source; choose and document whether that workflow-wide behavior remains rather than accidentally weakening it to only flagged trigger events.

**Tests.**

- Deliver concurrent events to multiple server processes for one `single_instance` trigger and prove that only one active launch is accepted.
- Park that instance at an approval and prove the lease is released; then create many independent suspended approvals without consuming active capacity.
- Race a due wake against a new trigger event and prove that whichever becomes active first owns the lease while the other follows the configured single-instance outcome.
- Crash after acquiring the lease and prove lease recovery is tied to durable launch state rather than a process lifetime.

**Corner cases.** A suspension can occur immediately after a launch starts, and a terminal event can arrive after the suspension transition. Use conditional generation/state updates so the lease is released once. Do not infer activity from the number of suspended rows: millions are expected and healthy.

**Risks and mitigations.** This changes current behavior, which checks only `running`. Publish the precise active-state definition, preserve the existing skip/ack behavior for duplicate trigger events, and add a reconciliation job for abandoned active leases after a crash.

### P1.7 — Pipeline attribution, alerting, and recovery visibility

**Implemented approach.** The pipeline API and sampler report durable launch
stages (`queued`, `leased`, `preparing`, `starting`, `active`, `expired`, and
`cancelled`), their counts and oldest age, runner/preparation capacity pressure,
precompile-child and child-reaping occupancy, and bounded workflow attribution.
The UI's stuck threshold comes from server policy rather than a separate
hard-coded value. Metrics remain low-cardinality; instance-specific diagnosis
uses logs or drill-downs.

Keep `parked` separate and slow-sampled. It is a large, healthy population, not a queue symptom. Operators act through conditional cancel/retry operations against a `launch_id`, never broad cleanup/deletion.

**Tests.**

- Build database fixtures covering every launch state and assert counts, oldest age, and terminal outcomes in the API and UI view model.
- Verify that a full runner with a moving queue, a stalled queue with no completions, a paused/draining Environment, and an expired lease are classified differently.
- Verify alert hysteresis and recovery clear conditions so a transient burst does not create repeated alerts.
- Load test a fast pipeline query with roughly one million suspended rows and a bounded launch set to prove parked approvals do not affect its cost.
- Authorize and race-test cancel/retry: a stale action against an old `launch_id` must not affect a newer generation.

**Corner cases.** A host clock can make oldest age negative or jump forward. Use server UTC timestamps and clamp display-only negatives. A large tenant can have many workflows; return bounded top contributors and paginate/drill down rather than inserting high-cardinality labels into metrics.

**Risks and mitigations.** Observability queries can become their own load and alerts can become noisy. Add indexes for active launch states, sample at a bounded rate, aggregate by workflow only in API responses, and require a duration threshold plus recovery hysteresis for notifications.

## Delivery sequence and gates

1. Add the policy types, ABI inventory, and read-only pipeline/outbox metrics. Inventory all legacy generated images and out-of-range timeout definitions before enforcing either policy.
2. Ship the durable launch schema, dispatcher, lease recovery, cancellation, and queue expiry behind a feature flag. Run it in observation mode first; do not route production starts through it until its counters match the existing path.
3. Route initial starts, resumes, wakes, cron, and every other admission source through the durable enqueue path. Remove direct semaphore waits only after the full-runner and restart tests pass.
4. Enable generation-safe supervision and preparation isolation, then make the supervisor the sole owner of active deadlines.
5. Complete the workflow compatibility cutover: rebuild invoke-ABI images, reject legacy and blocking workflow-agent shapes, then lower retry waits to durable suspension.
6. Enforce hard step/HTTP deadlines and add final pipeline alerting. Keep parked approvals explicitly outside all active-capacity and single-instance accounting.

Each gate needs a rollback that stops new dispatcher claims while preserving durable queue records. Never roll back by deleting launch records or by re-enabling the legacy workflow ABI.


## Policy values to decide before implementation

| Policy | Proposed starting point |
| --- | --- |
| Launch queue deadline | Five minutes, configurable per Environment. |
| Active execution timeout | Preserve the current one-hour product maximum initially, but enforce it server-side on every path. |
| Queue capacity | A durable admission bound plus a separate Environment-level pending-launch cap. |
| Retry and rate-limit budget | Never exceed the remaining active execution deadline; reject oversized values. |
| Workflow-as-agent waits | Reject until a versioned capability ABI can return a durable suspension outcome. |
| `single_instance` scope | Preserve the current workflow-wide semantics unless product explicitly narrows it; only queued, preparing, leased, starting, and running work are active. |
| Total workflow lifetime | Keep separate from active execution time. Parked approvals must remain exempt from runner, admission, and active-lease limits. |

## Acceptance tests

- When all runner slots are held, a large start batch leaves trigger and wake
  workers free, keeps the durable queue bounded, and never creates unbounded
  in-memory waiters.
- Cancelling or expiring a queued launch releases admission and never invokes
  the runner.
- Releasing one runner slot starts exactly one generation; stale cleanup cannot
  affect a resumed generation.
- A legacy direct-workflow artifact fails before it acquires a runner slot.
- A current-ABI approval parks, releases its runner permit and
  `single_instance` lease, and allows arbitrarily many suspended approvals.
