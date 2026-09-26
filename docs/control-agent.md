# Control agent for instance orchestration

Status: proposed implementation brief.

## Purpose

Let workflows start, inspect, coordinate, signal, and control independent Runtara
instances through a small set of platform capabilities. Keep orchestration in
workflows and let reports fetch data and submit actions through those workflows.

Independent instances make parallel approvals possible without requiring
parallel `WaitForSignal` steps inside one execution. A parent can start Finance
and Legal approval instances, each with its own wait, then wait for both outcomes.
Either approval can arrive first.

This is distinct from `EmbedWorkflow`: inline composition reuses workflow logic
inside an execution; starting an instance creates an independent lifecycle.

## Proposed capabilities

Every capability returns without suspending except `wait`, which suspends its
caller through the general agent suspension mechanism (see
[Agent suspension](#agent-suspension)).

Names are illustrative; finalize schemas during implementation.

| Capability | Inputs and behavior |
|---|---|
| `start` | Workflow ID, optional version, inputs, optional `runLabel`, and explicit ownership policy. Records the calling instance as the child's parent. Return the instance ID once the start is durably accepted, without waiting for completion. |
| `get` | Instance ID. Return status, suspension reason, relevant metadata, and terminal output or error when available. |
| `query` | Filter by workflow ID, label, statuses, date ranges, and optionally parent instance. Support deterministic sorting, pagination, and matching counts. |
| `wait` | Explicit instance IDs, mode `all` or `any`, and an optional deadline. Suspends the calling workflow until the condition holds or the deadline passes (see [Durable wait contract](#durable-wait-contract)). |
| `cancel` | Instance ID, optional reason, and grace period. Request cooperative cancellation with the platform's forced-abort fallback. |
| `pause` | Instance ID. Request an explicit pause; distinguish request acceptance from the target reaching the paused state. |
| `resume` | Instance ID. Resume an explicitly paused execution under existing lifecycle rules. This does not answer a `WaitForSignal` request. |
| `list-pending-signals` | Instance or workflow scope, supported filters, and pagination. Return open `WaitForSignal` requests with their signal IDs, request IDs, response schemas, and prompt/correlation context. |
| `send-signal` | Instance ID, signal ID (the target's `WaitForSignal` step signal), optional request ID, and payload. Validate and submit the response to that open request. Acceptance does not mean the target has finished handling it. |

## Signals

Control uses the `WaitForSignal` vocabulary: a workflow waits for a signal, and
another workflow sends it. There is one submission operation, and it is always
validated. It reuses the path behind the public `POST /api/runtime/signals`
endpoint and report actions (`submit_input_response`, core `submit_input` via
[workflow_runtime.rs](../crates/runtara-server/src/api/services/workflow_runtime.rs)):
schema validation, stale-request errors, and replay by operation ID. Reports
may keep presenting these requests as "actions"; they are the same requests.

`send-signal` addresses the signal ID the author gave the target's
`WaitForSignal` step, so a parent does not need runtime request IDs. The host
resolves it to the open request and submits with the operation identity it
derives from the calling step, so a replayed send cannot answer twice or
overwrite an accepted decision. It fails explicitly when:

- **Not waiting:** no open request has that signal ID, including one that has
  closed. This is the stale-request case.
- **Ambiguous:** several open requests share the signal ID (for example a wait
  inside a loop). The caller then passes a request ID from
  `list-pending-signals`.
- **Invalid:** the payload does not match the request's response schema.
- **Conflict / already answered:** the same operation with a different payload,
  or another operation answered first.

There is no raw, unvalidated signal capability. If one is ever needed, add it
under a clearly different name.

## Run labels

Labels follow the start-time contract in `runtara_dsl::run_label`
(`normalize_run_label`: printable ASCII with at least one non-space character).
Labels identify business context; instance IDs identify executions.

Raise the maximum length from 250 to 1024 bytes. Per-parent labels carry
composed business identities (keys, sub-steps, loop positions, retry suffixes,
for example `order-123/prepare/line-45/retry-2`), and 250 is tight for that.
1024 keeps every label-bearing btree index (the existing
`(tenant, run_label, created_at, instance_id)` lookup and the per-parent unique
index below) well under PostgreSQL's ~2.7 KB entry limit, so no hashed index is
needed; anything much beyond 2 KB would require one. The change needs
`MAX_RUN_LABEL_LENGTH`, a forward migration widening `instances.run_label`
(`VARCHAR(250)`, migration 025) and its check constraint (migration 029), and
the MCP and `runtara-dsl` README descriptions.

A label is unique per parent. When a workflow instance starts a child with a
label, `(parent instance, label)` identifies that child for the parent's whole
lifetime:

- Same parent, same label, same start operation (a replay after a crash):
  return the existing child. Replay must never fail on its own earlier start.
- Same parent, same label, a different start operation: reject with a label
  conflict error. The parent already has a child with that business identity.
- Different parents, or no parent, with the same label: allowed.
- Unlabeled starts are never checked for uniqueness.

A start operation's identity is deterministic: the start step plus its loop
position. The host service derives it from the invoking step (core already
records invocation paths in `invocation_attempts`); the agent never supplies
it, so a workflow cannot collide with or evade its own identity. A start step
inside Split or While must therefore produce a distinct label per iteration
(for example `order-123-prepare-{i}`); validation should warn when such a step
uses a constant label.

A label names one business attempt, whatever its outcome. A parent that
retries a failed child starts it under a new label (for example
`order-123-prepare-retry-1`); uniqueness does not lapse when a child finishes.
This keeps the parent's history unambiguous and is how retries compose from
the primitives.

Only workflow instances are parents in v1. Other starters keep their own
deduplication: channel and webhook intake identity, the HTTP `Idempotency-Key`,
and cron ticks. Report actions are a candidate for a later, general
`(starter kind, starter id)` scope.

Enforce this at admission, not at launch. `start` returns the child's ID once
the server durably accepts the request (`execution_requests`); the core
`instances` row that carries the run label today is created later, at launch.
A constraint there would reject a duplicate only after `start` had already
reported success. So `execution_requests` records the parent and enforces:

- the idempotency key `control:{parent instance}:{start operation}`, which
  returns the original child on replay through the existing idempotency path;
- a partial unique index on `(tenant, parent instance, run label)` where both
  are not null, which rejects a new start that reuses a label.

Admission records for a parent's children are retained under the same pinning
rule as the instances themselves, so both constraints stay authoritative for
the parent's lifetime.

## Parent link and result retention

Every instance started through Control records its parent instance on its
admission record and on the core `instances` row. The link serves three
purposes:

- **Retention.** A finished child stays readable while its parent is not
  terminal, however long the parent runs: a parent may `get` or `wait` on a
  child years after the child finished. Instance cleanup (today it deletes
  terminal instances after `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS`, 3 days by
  default) skips a terminal instance whose parent is not terminal. Once the
  parent finishes, its children fall back to normal retention.
- **Label scope,** as above.
- **Parent-close policy,** below.

Pinning is one level deep: a finished child can no longer read its own
children, so those follow normal retention. A pinned child only needs its
outcome (status, output, error, and the metadata `get` returns). Cleanup may
remove its checkpoints, step events, and debug data on the normal schedule and
keep the instance row with its outcome until the parent is terminal.

## Durable wait contract

`wait` takes explicit instance IDs, mode `all` or `any`, and an optional
deadline, and suspends its caller until the condition is satisfied or the
deadline passes. It is an ordinary Agent step using
[Agent suspension](#agent-suspension). `get` and `query` remain for callers
that prefer to poll.

Two limits apply and stay distinct:

- **The `wait` deadline (an input)** is a business timeout. When it passes,
  `wait` returns successfully with the observed outcomes and remaining IDs.
- **The step timeout** is the hard cap. When it passes, the step fails with a
  timeout error and follows onError.

Each suspension parks until the earlier of the two.

- `all` resolves when every target is terminal. Return each target's outcome.
- `any` resolves when at least one target is terminal. Return the observed
  finished targets and the remaining IDs; callers can repeat with the remainder.
- Terminal includes success, failure, cancellation, and execution timeout. A
  child's failure is outcome data, not automatically a retryable Control error.
- Empty target sets return an explicit empty result immediately. They do not
  subscribe to future matching instances.
- Already-terminal targets are observable immediately. Missing or inaccessible
  targets produce explicit errors rather than silently satisfying the wait.
- A wait deadline returns observed outcomes and remaining IDs. It does not
  cancel children. Resolve relative timeouts to a persisted deadline so replay
  cannot restart the timeout window.
- Persist the selected IDs and resolved outcome. Replaying a completed `any`
  must not choose a different result.
- Waiting must release the parent's execution resources and survive restarts.
  Do not hold a WASM invocation, worker slot, or in-memory polling task for the
  lifetime of a business wait.
- Completion between checking targets and registering a wait must not be lost.
  Use durable notification/subscription machinery or restart-safe scheduled
  polling with a race-safe check/park protocol.
- Result retention must allow unresolved waits to observe target outcomes.
  Children are pinned by their parent link. In v1 `wait` accepts only the
  caller's own children and rejects any other target: letting a workflow keep
  arbitrary instances alive raises authorization and retention questions v1
  does not need. Define behavior for deleted targets; never wait indefinitely for
  missing data.

The existing live-task observation helper in
[runtime_client.rs](../crates/runtara-server/src/runtime_client.rs) is not itself
a durable workflow wait. `wait` gets durability from typed agent suspension
(see [Agent suspension](#agent-suspension)): the caller parks without holding a
Store or worker slot, and the step's result checkpoint fixes the outcome.

Querying discovers instances. Waiting coordinates a fixed set of IDs. New runs
matching a label must not silently expand an existing wait.

## Replay and lifecycle guarantees

Every logical start needs a stable operation identity. If a parent crashes after
the child start is accepted but before saving its result, retrying must recover
the same child ID. Do not rely solely on checkpointing after the side effect.
Persist the resolved workflow version so replay cannot select a new `latest`.
A replayed start must also carry the same inputs, workflow, and version as the
original; a mismatch is a conflict, not a silent return of the old child.

Signal submissions need replay protection. Define duplicate
and conflicting response behavior explicitly; a replay must not overwrite a
previously accepted decision. Lifecycle commands must tolerate repeated delivery
and races with terminal transitions.

Every child records its parent (see above). Parent-close policy decides what
happens to children still running when the parent terminates:

- Leave the child running when the parent terminates.
- Request child cancellation when the parent terminates.

Apply the policy durably even if the parent crashes or is stopped from outside;
in that case no parent code runs, so the platform must cascade it through the
parent link. The parent's own error handling can always cancel children it
holds IDs for. Parent pause propagation
must be explicit; pausing a parent does not implicitly pause all related work.
An `any` result or wait timeout does not implicitly cancel remaining children.
Cancellation is not rollback of business side effects.

## Supported compositions

| Pattern | Composition |
|---|---|
| Parallel approvals | Start independent approval instances, wait `all`, inspect decisions. |
| First result wins | Start alternatives, wait `any`, optionally cancel the remaining instances. |
| First successful result | Wait `any`, inspect outcomes, repeat on remaining instances after failures. |
| Process results as they arrive | Wait `any`, process returned outcomes, repeat with the remaining IDs. |
| Bounded concurrency | Start N instances, wait `any`, start replacements until the workload is exhausted. |
| Detached background work | Start with leave-running ownership policy and return. |
| Approval deadline | Wait with a deadline, then explicitly escalate or cancel. |
| Approval inbox | List pending signals, render their data, and answer through `send-signal`. |

Batch start, quorum, retries, and compensation do not need separate initial
capabilities; workflows can compose the primitives. Sequential submission of
non-blocking starts still allows the resulting instances to execute concurrently.

## Exposure

Operations reach the agent through a typed host interface, following the
database service pattern (`runtara-workflow-wit/wit/database`, `DatabaseHost`,
`NativeDatabase`, `dispatcher.set_database`):

- **WIT:** a `runtara:control` interface with `start`, `get`, `query`,
  `list-pending-signals`, `send-signal`, `cancel`, `pause`, and `resume`,
  taking typed records.
- **Host trait:** `ControlHost` in `runtara-component-host`.
- **Server implementation:** `NativeControl`, calling `ExecutionEngine::queue`,
  the executions list, `get_instance_info`, core `submit_input`, and
  Environment stop, pause, and resume.
- **Wiring:** `dispatcher.set_control(...)` in `server.rs`.
- **Agent:** `crates/agents/runtara-agent-control` is a thin component that
  validates input, calls the import, and shapes output. Being an agent gives it
  catalog metadata, step-editor schemas, and MCP discovery.

Authority comes from the host, never from function arguments. No host function
takes a tenant, parent, or operation ID; the host reads them from the call
context:

- **Tenant** and **parent** (the calling instance) are already in
  `CallContext`.
- **Operation identity** is missing: `CallContext` carries only tenant,
  instance, and core URL. The invoking step's compiler checkpoint key (step plus
  loop position) must be passed into the agent call so the host can build
  `control:{parent}:{operation}` for `start` and the operation ID for
  `send-signal`.
- **Caller identity** is added when the authorization model is decided.

This composes with existing replay: a completed step's result is checkpointed,
so replay does not call the host again, and a crash between the host call and
the checkpoint is caught by the host-side idempotency key.

Link `runtara:control` only into the operator-installed, digest-pinned
built-in control agent, as the trusted executor restricts itself to approved
built-ins. Other agents, including third-party ones, must not be able to start
or control instances.

`wait` is not a blocking host function: that would hold the Store and worker
slot for the whole wait. The host function only checks or registers the wait;
the capability then returns a suspension (below).

## Agent suspension

Any agent capability may suspend its calling workflow if it declares so.
`wait` is the first user; others could include waiting on an export, an
external batch job, or a long-running provider operation instead of holding a
worker.

Today only a composed workflow-agent can suspend its caller, through the
reserved `__rt_suspended__` error code; any other agent or user error carrying
it is remapped so it fails (`runtara-workflow-stdlib/src/direct_json.rs`).
Replace that sentinel with a typed contract:

- **Typed result.** The agent WIT gains a suspend result alongside success and
  error: a wake set plus an opaque continuation state blob. Suspension is
  never encoded as an error.
- **Wake sources.** A timer (`at`) and instances becoming terminal (host-owned
  wait records). Not signals: registering input requests belongs to the
  workflow runtime, which agents do not import.
- **Continuation state.** The host stores the state blob under the step's
  operation identity and passes it back on the next invocation. On wake the
  parent replays to the same step and calls the agent again with the same
  input; the state tells it where it left off, so work done before suspending
  is not repeated. The blob is size-capped; it is a continuation, not general
  storage.
- **Declared in metadata.** A capability declares `suspends: true` in its
  `.meta.json`. The compiler emits suspension handling only for such steps,
  the UI shows them as long-running, and validation checks their context.
- **Durable and bounded.** A suspending step must be durable and must set a
  step timeout; validation rejects either missing. The durable step timeout is
  already an absolute, persisted epoch deadline that counts parked time and is
  shared across retries (`runtara-workflows/src/direct_wasm/compile/agent_deadline.rs`),
  so it bounds the whole suspension. No separate platform maximum is needed.
- **Supported contexts.** Validation must reject, or the runtime must support,
  suspending steps inside Split/While, parallel branches, AiAgent tool calls,
  embedded children, retries, and onError. Composed workflow-agents already
  suspend in some of these; the set must be explicit.
- **Completion sticks.** Once the capability returns success, the normal
  agent-step result checkpoint fixes the result, so replay returns it without
  invoking the agent again (for `wait`, this keeps an `any` result stable).

Long suspensions (a parent waiting a year) additionally require:

- **Wakes from durable data.** The terminal-path hook records the wake in the
  same transaction as the target's terminal status, or a reconciler rescans
  open waits. An in-process notification lost in a restart would strand the
  waiter.
- **Resumable artifacts.** The compiled package for a suspended instance's
  pinned workflow version must be kept while any instance of it is suspended,
  and host ABI changes must keep such packages resumable or migrate them. This
  applies equally to long `WaitForSignal` waits today.
- **The default runner.** Only the embedded runner parks; the `CliRunHttp` ABI
  still blocks, so suspending capabilities require the embedded runner.

## Implementation boundaries

- Put the workflow agent under `crates/agents/runtara-agent-control`, following
  component build and metadata conventions. Do not place it in host-only
  `crates/runtara-agents`.
- Supply platform access only through the `runtara:control` host interface
  (see [Exposure](#exposure)). Agent code must not receive platform
  credentials, unrestricted internal HTTP, database access, or raw filesystem
  access to implement these operations.
- Derive tenant and caller identity from authoritative host context. Enforce
  target authorization on every operation, including waits and queries.
- Reuse Server workflow admission/version resolution, Core durable state and
  signals, and Environment lifecycle/launch/wake behavior. Starting by workflow
  ID must not bypass the normal compilation and launch path.
- Implement waiting through typed agent suspension (see
  [Agent suspension](#agent-suspension)). Do not represent durable suspension
  as an ordinary retryable agent failure.
- Keep public outcomes distinct: operation accepted, instance terminal, child
  failed, and observer timed out are different results.
- Query filters apply before pagination. Execution status and suspension reason
  are distinct from arbitrary business-stage or checkpoint-state queries.

## Current platform state

Checked against `main` on 2026-09-26.

Reusable server-side operations:

- `start`: `ExecutionEngine::queue` persists the resolved version and a durable
  idempotency key. Gap: a reused key only detects run-label mismatches; other
  input, workflow, or version differences silently return the old run.
- `get` / `query`: `get_instance_info`; the executions list filters by
  workflow, run label, statuses, and dates in SQL before pagination, with
  counts. Gaps: no parent filter; no suspension reason in list results.
- `cancel` / `pause` / `resume`: Environment stop already signals cooperative
  cancellation and force-aborts at a deadline. Gap: the server wrapper
  hard-codes a 5-second grace and a fixed reason.
- `list-pending-signals` / `send-signal`: `workflow_runtime.rs` and core
  `submit_input` provide schema validation, stale-request errors, replay by
  operation id, and conflict/already-answered codes. Gaps: no run-label filter
  on listing; submission is addressed by request ID, so resolving a signal ID
  to its open request (and the ambiguous case) is new.
- `RuntimeClient::send_custom_signal` (unvalidated raw custom signals) has no
  callers in the server; the public signals endpoint already uses the
  validated path. Treat the wrapper as dead code rather than a building block.

Missing:

- **Host service.** Agents can import only timers, typed outbound HTTP, the
  trusted executor, the connection resolver, and SQL. Add a control WIT
  interface, host trait, and server implementation following the database
  service pattern (`runtara-workflow-wit/wit/database`, `DatabaseHost`,
  `NativeDatabase`), linked only into the built-in control agent.
- **Operation identity in the call context.** `CallContext` has no invoking
  step identity; the step's compiler checkpoint key must reach the host.
- **Caller identity.** Only the tenant reaches the component host;
  `QueueRequest` carries no user. Authorization beyond tenant scope needs this
  plumbing and a policy.
- **Parent link.** Neither core `instances` nor server `execution_requests`
  has a parent column. Workflow-as-agent runs children inline in the same
  execution and does not provide one. Run labels are stored on `instances` with
  a non-unique index only.
- **Durable wait.** Store-freeing suspension is on by default, and parking,
  persisted deadlines, and the check-then-park self-wake exist for signals.
  Missing: a wake source for another instance becoming terminal, a durable
  wait/subscription row, a hook on the terminal path that re-evaluates waits
  (the existing `on_terminal` hook is metrics-only and drops errors), typed
  agent suspension with continuation state (only compiler-emitted steps and
  composed workflow-agents can suspend today, via an error sentinel),
  `suspends` capability metadata and its validation, retention pinning, and
  artifact retention for suspended instances.

Related: the component host exports `RUNTARA_HTTP_URL`, `RUNTARA_TENANT_ID`,
and `RUNTARA_INSTANCE_ID` into every guest environment
(`runtara-component-host/src/host_state.rs`). Raw `wasi:http` is denied, but a
typed control service makes the internal URL unnecessary to expose.

Before building `wait`, prototype parallel approvals on existing signals: each
approval instance signals the parent on completion, and the parent waits on
each signal in turn. If the self-wake picks up a signal that arrived first,
this covers the headline use case without new wake machinery.

## Decided

See [control-agent-decisions.md](control-agent-decisions.md) (2026-09-26):
- cross-lineage `send-signal` requires opt-in via `action.key`;
- host verification of composed bytes ships in v1;
- a parent-close policy is required, with `cancel` preselected;
- pausing a waiting run pauses it immediately;
- control is on every tier; its children count against the concurrency limit (parked runs don't), with at most 80% of it usable by control;
- lineage depth is capped at 16;
- `start` fails fast on missing or broken workflows;
- the parent appears in the public API.

## Decisions before implementation

- Final capability schemas, pagination limits, output-size handling, and the
  representation of per-instance outcomes.
- Parent-close policy defaults and which parent terminal outcomes trigger it;
  whether v1 needs explicit pause propagation or leaves children unaffected.
- Which pinned child data is kept beyond the outcome, and how long a child is
  kept after its parent becomes terminal.
- Continuation state size cap, and which contexts support suspending steps in
  v1 (Split/While, parallel branches, AiAgent tool calls, embedded children).
- Durable wait implementation, completion/deadline tie behavior, and outcome
  retention while parents are waiting.
- Stable operation identities, conflicting replay behavior, and atomic handling
  of competing approval submissions.
- Whether existing cancellation with a grace period is sufficient for v1. Add a
  separate force-terminate operation only if that distinct contract is needed.

## Acceptance criteria

- A parent starts two approval instances, parks, and resumes after both finish
  in either order. The parent does not retain a live execution slot while parked.
- Crashes before and after start acknowledgement do not create duplicate children.
- A parent starting the same label twice gets a conflict error at `start`, never
  a later launch failure; a replayed start returns the original child; another
  parent may use the same label; a retry under a new label succeeds.
- `wait` rejects targets that are not the caller's children.
- A capability declaring `suspends: true` is rejected by validation on a
  non-durable step or one without a step timeout, and in unsupported contexts.
- A suspended caller resumes after a server restart; the agent receives the
  continuation state it returned and does not repeat work done before
  suspending. The step timeout fails the step even while it is parked, and the
  `wait` deadline returns partial outcomes without failing.
- A suspended instance's compiled package survives artifact cleanup until the
  instance is terminal.
- A child that finished long ago stays readable through `get` and `wait` while
  its parent runs, and becomes eligible for cleanup once the parent is terminal.
- Waits handle all terminal outcomes, empty sets, already-finished targets,
  deadlines, and missing targets according to the contract.
- A child finishing during wait registration cannot strand the parent. Replayed
  waits retain their original target set, deadline, and resolved results.
- `any` leaves unrelated children running; cancellation occurs only through an
  explicit command or configured ownership policy.
- Parent-close policies survive restarts. Pause/resume remains distinct from
  signal delivery, and terminal-state races have defined outcomes.
- `send-signal` enforces schema validation, stale-request handling, replay
  protection, and tenant/caller authorization; it resolves a signal ID to its
  single open request and reports not-waiting and ambiguous cases explicitly.
- Queries find matching instances beyond the first page, including label-filtered
  runs, with correct counts.
- Run focused Core/Environment/Server tests, component builds and relevant
  integration tests, plus compiler/runtime suspension and replay tests. Regenerate
  affected contracts through existing tooling and use forward SQL migrations.

This brief does not implement the report redesign, general parallel execution of
embedded graphs, subscriptions to future query matches, or business compensation.
