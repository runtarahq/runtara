# Control agent for instance orchestration

Status: implemented. [As built](#as-built) is the reference for authors and
operators; the sections after it are the original brief, kept for its
rationale. Owner decisions D1-D8 in
[control-agent-decisions.md](control-agent-decisions.md) override the brief.

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

## As built

This section is the author reference. The same text is served to MCP clients
as `controlAgent` in `get_workflow_authoring_schema`
(`crates/runtara-server/src/mcp/tools/workflows.rs`); its numbers come from
`runtara-control-contract` and the `InstanceWaits` service.

Waiting on children is not a control capability: it is the
[WaitForInstances](#waitforinstances) step, which parks the run itself.

### Capabilities

The `control` agent is on every pricing tier, whatever the agent allowlist
says. Its mutations run only as steps of a workflow run
(`runtime:requires-run`): `test_capability` answers
`CONTROL_REQUIRES_INSTANCE` for them.

| Capability | What it does |
|---|---|
| `start` | Durably admit a child run of another workflow; returns `{instanceId, workflowId, version, runLabel, replayed}` once accepted, without waiting for it. |
| `get` | Read one run of the tenant: status, `suspensionReason`, `parentInstanceId`, output or error. |
| `query` | Page runs of the tenant by `createdAtMs` or `finishedAtMs`; `parentInstanceId` or `callerChildren` lists children, including ones still `queued` in admission. |
| `list-pending-signals` | List open WaitForSignal requests of one run, a workflow, or the calling run's children. |
| `send-signal` | Answer the one open request of a WaitForSignal step, validated against its response schema. |
| `cancel` | Cancel a direct child: cooperatively, forced after `graceMs`. A parked or queued child ends at once. |
| `pause` | Pause a direct child. A waiting child pauses at once, a running one at its next checkpoint. |
| `resume` | Resume an explicitly paused direct child (`CONTROL_NOT_PAUSED` otherwise). It never answers a WaitForSignal request. |

### Authorization (D1)

- Reads (`get`, `query`, `list-pending-signals`) see every run of the tenant.
  The caller-relative filters (`query callerChildren`,
  `list-pending-signals children`) need a calling run.
- `cancel`, `pause` and `resume` reach direct children only:
  `CONTROL_NOT_CHILD` otherwise, `CONTROL_DENIED` for an ancestor. A
  WaitForInstances step follows the same rule with `INSTANCE_WAIT_*` codes.
- `send-signal` answers a child, an ancestor, or any run whose WaitForSignal
  request opted in with `action.key` when the step passes the same
  `actionKey`; anything else is `CONTROL_DENIED`.
- No mutation may target the calling run (`CONTROL_INVALID`). Tenant, caller
  and operation come from the host, never from step inputs.

### start (D6, D7)

- **Inputs:** `workflowId` (an id, not a slug), optional `version` (default:
  the current version, fixed at admission), `inputs` (`{data, variables}`,
  validated against the child's input schema), optional `runLabel`, and a
  required `parentClosePolicy` (`cancel`, which the editor preselects, or
  `leave_running`).
- **Run label:** at most 1024 bytes of printable ASCII. It names one child per
  parent for the parent's whole lifetime: reusing it from another step or
  iteration is `CONTROL_LABEL_CONFLICT`, and a retry of a failed child needs a
  new label. Other parents, and runs without a parent, may reuse it.
- **Depth:** lineage is capped at depth 16 (a top-level run is depth 1): a run
  at depth 16 cannot start a child (`CONTROL_INVALID`).
- **Fast failure:** a missing workflow or version is `CONTROL_NOT_FOUND` and a
  permanently failed compilation `CONTROL_NOT_RUNNABLE`. A workflow not
  compiled yet is admitted and launches once it compiles, until the admission
  deadline (then the child fails with `launch_deadline_not_compiled`).
- **Parent link:** the child records the calling run as `parentInstanceId`,
  reported by `get`, `query`, the executions API and the invocation history.

### Capacity (D5)

- Children count against the tenant concurrency limit
  (`MAX_CONCURRENT_EXECUTIONS`, or the `maxConcurrentExecutions` entitlement if
  lower), which counts only starting and running runs: a parked run gives its
  slot back.
- Control-started children may hold at most `max(1, floor(0.8 x limit))`
  slots (limit 10 gives 8, limit 5 gives 4), so outside triggers keep
  headroom.
- At the share, `start` fails with retryable `CONTROL_CAPACITY_RATE_LIMITED`
  carrying a retry hint of 3-8 s; set `maxRetries` on the step to try again.
- With a limit of at most 1 the calling run holds the only slot, so `start`
  fails permanently with `CONTROL_CAPACITY_UNSATISFIABLE` (the server warns at
  boot).

### Statuses

- Values: `queued`, `pending`, `running`, `suspended`, `completed`, `failed`,
  `cancelled`, `not_started`.
- `queued`: a child still in admission, accepted by `start` and not launched
  yet. It can be cancelled but not paused or resumed.
- `not_started`: a child that never launched; its one fenced outcome is
  `not_started` or `cancelled`. `get` and `query` report it; the public
  executions list does not.
- `suspensionReason` of a suspended run: `paused` (explicitly paused, only a
  resume relaunches it), `waiting_signal`, `waiting_instances` (a
  WaitForInstances step), `sleeping` or `shutdown`. The executions API
  reports the same as `WorkflowInstanceDto.suspensionReason`.
- Terminal: `completed`, `failed` and `cancelled` (including execution
  timeout). A child's failure is outcome data, never a retryable control
  error.

### WaitForInstances

A step type, next to WaitForSignal, displayed as "Wait for Instances":

```json
{ "stepType": "WaitForInstances", "id": "waitApprovals",
  "instanceIds": { "valueType": "composite", "value": [
    { "valueType": "reference", "value": "steps.finance.outputs.instanceId" },
    { "valueType": "reference", "value": "steps.legal.outputs.instanceId" } ] },
  "mode": "all",
  "timeoutMs": { "valueType": "immediate", "value": 86400000 } }
```

- **Fields:** `instanceIds` (direct children, at most 1000 distinct), `mode`
  `all` (default) or `any`, optional `timeoutMs` (from the first time the step
  runs), optional `breakpoint`. No durable, retry or step-timeout fields.
- **Output** at `steps.<id>.outputs`: `{mode, resolution: satisfied |
  deadline | empty, finished: [{instanceId, status, finishedAtMs, output,
  outputBytes, outputOmitted, error, errorOmitted}], remaining: [ids],
  deadlineMs}`.
- `all` settles when every target is terminal, `any` when at least one is;
  repeat `any` with `remaining` to process results as they arrive.
- An empty `instanceIds` settles at once with resolution `empty`. Already
  finished targets count at once.
- Targets are checked before anything registers (D1): the run itself is
  `INSTANCE_WAIT_INVALID`, an ancestor `INSTANCE_WAIT_DENIED`, another run
  `INSTANCE_WAIT_NOT_CHILD`, an unknown id `INSTANCE_WAIT_NOT_FOUND` (D7), more
  than 1000 `INSTANCE_WAIT_TOO_LARGE`. Other codes: `REPLAY_CONFLICT`,
  `CLOSED`, `UNAVAILABLE`, `FAILED`, all prefixed `INSTANCE_WAIT_`.
- `timeoutMs` is a business deadline: the step settles with resolution
  `deadline` and what finished; it never cancels children and is not an
  error. The first registration's deadline is persisted and wins on replay.
- The run parks without holding a runner or a concurrency slot and survives
  restarts; the wake is recorded in the same commit as the child that
  satisfies it.
- The wait registers once per step and loop position: a replayed `any` keeps
  its choice, and other targets or mode are `INSTANCE_WAIT_REPLAY_CONFLICT`.
- The workflow must be durable (E028). The step may not sit in onError, a
  WaitForSignal `onWait`, or an AiAgent tool or memory target (E131). Literal
  `instanceIds` or `timeoutMs` out of range are E133.

### Limits

| Limit | Value |
|---|---|
| Control call input | 1 MiB |
| `get` inlined output / error | 1 MiB / 64 KiB, else `outputOmitted`/`errorOmitted` with the size |
| Page size (`query`, `list-pending-signals`) | 1-100, default 20 |
| WaitForInstances targets | 1000 |
| WaitForInstances inlined output / error per child, total | 256 KiB / 16 KiB, 3 MiB per wait; larger values are omitted and flagged |
| Run label | 1024 bytes |
| Lineage depth | 16 |
| `cancel` grace | 0-3600000 ms, default 5000 |
| Parent-close grace | 5000 ms |
| One control call | 90000 ms, below the step's own timeout (`CONTROL_TIMEOUT`) |

### Replay

Each control step call has an operation identity: the step plus its loop
position. A retried attempt, a crash recovery and a resumed run replay the
same identity.

| Capability | On replay |
|---|---|
| `get`, `query`, `list-pending-signals` | A durable step returns its checkpointed result; a non-durable one reads again. |
| `start` | Returns the same child (`replayed: true`), also after a crash between admission and the checkpoint. Other arguments are `CONTROL_REPLAY_CONFLICT`. |
| `send-signal`, `cancel`, `pause`, `resume` | Returns the receipt of the first call (`replayed: true`) without acting again. Other arguments are `CONTROL_REPLAY_CONFLICT`. |
| WaitForInstances | Reads the wait it registered: same targets, deadline and, for `any`, the same choice. Other targets or mode are `INSTANCE_WAIT_REPLAY_CONFLICT`. |

### Validation

| Code | Meaning |
|---|---|
| E028 | A WaitForInstances step or a suspending agent step is not durable. |
| E029 | A suspending agent step has no timeout, or timeout 0. |
| E131 | A WaitForInstances step or a suspending agent step sits in an onError region, a WaitForSignal `onWait`, or an AiAgent tool or memory target. |
| E132 | A control step sits in a WaitForSignal `onWait`, or an AiAgent tool or memory target. |
| E133 | A WaitForInstances step's literal `instanceIds` (empty, not string ids, over 1000) or `timeoutMs` (not a positive integer) is invalid. |
| W073 | A Split's `parallelism` is ignored: its body holds an operation-scoped step (or another shape that forces sequential execution). |
| W074 | A control `start` in a Split or While uses a literal `runLabel`; the second iteration fails with `CONTROL_LABEL_CONFLICT`. |
| W075 | A control, WaitForInstances or suspending step in a parallel Split or an unconditioned branch group; that region runs serialized. |
| W076 | A control, WaitForInstances or suspending step under a retrying Split or EmbedWorkflow; a region retry replays the operation's first outcome. |
| W077 | A control `start` whose `workflowId` is not a literal; the target is only checked when the step runs. |
| W078 | A suspending agent step's timeout is at most 1000 ms, so it times out instead of parking. |

No built-in agent suspends today; agent suspension remains for future
long-polling agents and may wake only on a timer (`at`). An agent asking to
wake on instances is refused with `AGENT_INVALID_SUSPENSION`.

A workflow with control, WaitForInstances or suspending steps (or embedding
one) may be published as a workflow-agent. The workflow-agent runs inside its
caller's instance: the runs it starts are children of the caller, its
WaitForInstances targets are those children, and a park crosses to the caller
as the `suspended` outcome. The full context matrix is
`operationScopedSteps` in the authoring schema.

### Lifecycle (D3, D4)

- `parentClosePolicy: cancel` cancels a still-running child whenever its
  parent ends (completed, failed, cancelled, or gone), with a 5 s grace, even
  when the parent crashed or was stopped from outside; `leave_running` leaves
  it alone. A suspended parent has not ended.
- Pausing a waiting run (parked on a timer, a signal or its children) pauses
  it at once, in control and in the public API; it loses its wake, and only
  an explicit resume relaunches it. Signal answers and finished children are
  kept and seen after the resume.
- Pausing a parent does not pause its children, and neither an `any` result
  nor a WaitForInstances deadline cancels the remaining children; cancel them explicitly.
- A finished child stays readable through `get` and WaitForInstances until its parent
  is terminal (one level deep), then follows normal retention.
- Cancellation is not rollback of the child's business side effects.

### Executor model

The copy of the control agent composed into a workflow only forwards: its
calls go to `runtara:control/executor`, which the host binds to its
`ControlExecutor`. The executor instantiates the host-installed control bytes
(never the composed copy, never tenant bytes) in a fresh restricted store per
call (64 MiB memory, 90 s, at most 16 at once) where `runtara:control/api` is
real; everywhere else that interface is `denied`. Tenant, caller and step
operation come from the calling workflow's store, never from the agent's
input.

Decision D2 is enforced twice. When an artifact is prepared, the precompile
worker audits every composed component that imports `runtara:control/*`; the
artifact binds its one `runtara:builtin-artifacts/control-…` pin plus those
digests. Every call re-checks that binding and the executor's own bytes
against the approved history. The server approves its bundle's control bytes
at boot (`approved_builtin_artifacts`, never deleted); an operator revokes a
version by setting `revoked_at`, effective at the next boot. After that, calls
through the revoked version are `CONTROL_DENIED`, new launches of workflows
pinning it are not ready, and runs already parked on it still load and fail at
their next control call. A run pinned to an older, still approved version
keeps working after an upgrade.

Trusted built-ins (S3, Azure presigning) share that history (trusted pins,
option B). Boot records the installed `runtara:trusted-artifacts/…` pins in
`approved_builtin_artifacts` too. A trusted call from a workflow whose
artifact pins an older version of that agent is admitted only when the run
continues a parked run, that is its launch is a wake or a resume (paused
waits included), and only if that older pin is approved and not revoked; it
then runs the installed bytes. A start under an older pin, and any launch
under a revoked or never approved pin, fails with `TRUSTED_VERSION_REQUIRED`
before any credential is resolved. The launch kind comes from the durable
launch queue row through the host's runtime object, never from the guest, and
defaults to start. Compilation readiness still needs the installed pin, so new
runs recompile. Why this is safe: the pin never chooses the bytes (only
installed, operator-approved bytes run), credentials are still resolved by
tenant, connection and type for the calling run, a start can never use the
history, and revocation (effective at the next boot) is the operator's switch
to cut off runs parked on a version. Composed artifacts whose other agents import
`runtara:control` or `runtara:workflow-operation` are refused at preparation.

Mutations run inside a compiler-emitted operation scope
(`runtara:workflow-operation`), which gives each call site its operation
identity. A WaitForInstances step is compiled workflow code, not agent bytes:
it calls the `runtara:workflow-wait@0.1.0` host interface (`register`,
`poll`, `release`) with a key built from the step and its loop position, and
the host's `InstanceWaits` service owns the wait. Workflows without control,
WaitForInstances or suspending steps compile to the same bytes as before.

### Retention

- **Children:** instance cleanup keeps a finished child until its parent is
  terminal and both are past `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS` (one level
  deep; a missing parent counts as terminal) and logs
  `pinned_terminal_children` per pass. The fenced outcome of a child that never
  launched follows the same rule.
- **Pruning pinned children:** after deleting, the same pass prunes every
  child it had to keep only because its parent is still live (terminal, past
  retention by its own finish, parent not terminal): it drops the child's
  checkpoints, lifecycle and custom signals, closed input requests, input
  park, invocation lease and attempts, and clears `input` and `stderr`. The
  `instances` row stays with its outcome (status, output, error, termination
  reason, parent link, run label, metadata), as do its events and accepted
  input receipts, so `get`, WaitForInstances and a replayed `send-signal` answer as
  before. Keyset cursor, one transaction per `RUNTARA_DB_CLEANUP_BATCH_SIZE`
  page, no new setting; logs `pruned_children`, and a rerun prunes nothing.
  Pruned checkpoints and input are no longer inspectable. Cleanup does not delete admission rows
  (`execution_requests`), so the per-parent label and replay constraints hold
  for the parent's lifetime.
- **Per-run state:** command receipts (`instance_control_receipts`), waits
  (`instance_waits`, `instance_wait_targets`) and suspension state
  (`instance_agent_continuations`) belong to the calling run and are deleted
  with it.
- **Artifacts:** image cleanup keeps the compiled package of a run that is
  parked or still pinned, and never removes an image a launch still
  references, so a parent parked for months resumes on the artifact it was
  built from. Released host interfaces stay linked (see the
  `runtara-workflow-wit` README's ABI rule).
- **Audit:** every control mutation attempt writes an `audit_events` row
  (`control.start`, `control.send_signal`, `control.cancel`, `control.pause`,
  `control.resume`) with its caller, operation and outcome,
  never its payload.

### Debugging

- The execution detail shows **Started by** (the parent) and **Child runs**;
  the invocation history has a Parent column and a `parentInstanceId` filter
  (`GET /api/runtime/executions?parentInstanceId=`, MCP `list_executions
  parent_instance_id`).
- A parked run reads `suspended` with `suspensionReason` (`waiting_instances`
  for a WaitForInstances step). Its unfinished step reads `suspended` in
  `GET /api/runtime/workflows/{id}/instances/{iid}/steps` (and MCP
  `get_step_summaries`); `status=suspended` filters for it. A step reports one
  row however often a resumed run replays it.
- Step failures carry `CONTROL_*` or `INSTANCE_WAIT_*` codes (see `errorCodes` in the authoring
  schema); `replayed: true` in a step output means the call was answered from
  its receipt, not repeated.
- `control:get` or the API show why a child ended; a child cancelled by the
  parent-close cascade records the reason `parent <id> terminated (<status>)`.
- Operator tables: `execution_requests` (server database: admission,
  `parent_instance_id`, per-parent labels), `instance_waits` and
  `instance_wait_targets` (open waits and their targets),
  `instance_external_outcomes` (never-launched children),
  `approved_builtin_artifacts` (the approved control history).

## Proposed capabilities

The brief proposed a `wait` capability that suspended its caller through
[Agent suspension](#agent-suspension). As built, waiting is the
[WaitForInstances](#waitforinstances) step and every control capability
returns without suspending.

The shipped names and schemas are in [As built](#as-built).

| Capability | Inputs and behavior |
|---|---|
| `start` | Workflow ID, optional version, inputs, optional `runLabel`, and explicit ownership policy. Records the calling instance as the child's parent. Return the instance ID once the start is durably accepted, without waiting for completion. |
| `get` | Instance ID. Return status, suspension reason, relevant metadata, and terminal output or error when available. |
| `query` | Filter by workflow ID, label, statuses, date ranges, and optionally parent instance. Support deterministic sorting, pagination, and matching counts. |
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
  terminal, however long the parent runs: a parent may `get` or wait on a
  child years after the child finished. Instance cleanup (it deletes
  terminal instances after `RUNTARA_DB_CLEANUP_MAX_AGE_DAYS`, 3 days by
  default) skips a terminal instance whose parent is not terminal. A pinned
  child becomes eligible once its parent is terminal and both are past
  retention: its age counts from the later of the two finishes. A missing
  parent counts as terminal. The published outcome of a child that never
  launched follows the same rule, aged from its publication.
- **Label scope,** as above.
- **Parent-close policy,** below.

Pinning is one level deep: a finished child can no longer read its own
children, so those follow normal retention. A pinned child only needs its
outcome (status, output, error, and the metadata `get` returns). Once it is
past retention by its own finish, cleanup prunes it (see Retention above):
checkpoints, signals, closed input requests, invocation state, `input` and
`stderr` go; the instance row with its outcome, its events and its accepted
input receipts stay until the parent is terminal. Step-debug events age out
on their own window as for any run.

## Durable wait contract

The WaitForInstances step takes explicit instance IDs, mode `all` or `any`,
and an optional `timeoutMs`, and parks its run until the condition is
satisfied or the deadline passes. (The brief made this a control `wait`
capability on [Agent suspension](#agent-suspension); it became a step because
it is the run's own control flow, not an operation on another run.) `get` and
`query` remain for callers that prefer to poll.

The deadline is a business timeout. When it passes, the step settles
successfully with the observed outcomes and remaining IDs. The step has no
separate hard timeout.

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
  Children are pinned by their parent link. In v1 a wait accepts only the
  caller's own children and rejects any other target: letting a workflow keep
  arbitrary instances alive raises authorization and retention questions v1
  does not need. Define behavior for deleted targets; never wait indefinitely for
  missing data.

The existing live-task observation helper in
[runtime_client.rs](../crates/runtara-server/src/runtime_client.rs) is not itself
a durable workflow wait. WaitForInstances registers its wait in the
`instance_waits` tables and suspends the run with its deadline as the only
timed wake; the host attaches the pending wait and the runner parks with
`ParkReason::Instances`, holding no Store or worker slot. After the wake the
replay reaches the step, finds the settled wait, and the step's checkpoint
fixes the outcome before the wait row is released.

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
| Parallel approvals | Start independent approval instances, WaitForInstances `all`, inspect decisions. |
| First result wins | Start alternatives, WaitForInstances `any`, optionally cancel the remaining instances. |
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

Waiting is not a blocking host function: that would hold the Store and worker
slot for the whole wait. The host function only checks or registers the wait;
the WaitForInstances step then suspends the run.

## Agent suspension

Any agent capability may suspend its calling workflow if it declares so. It
is an extension point for long-polling agents (waiting on an export, an
external batch job, or a long-running provider operation instead of holding a
worker); no built-in agent uses it since control `wait` became the
WaitForInstances step, and an agent may now only wake on a timer (`at`).

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
  invoking the agent again.

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
- Implement waiting as the WaitForInstances step (the brief said typed agent
  suspension). Do not represent durable suspension as an ordinary retryable
  failure.
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

### Signals-only parallel approvals (S0.1)

The prototype suggested before building a wait was: each approval run signals
the parent when it finishes, and the parent waits on each signal in turn. It
does not work, because an early signal is refused rather than kept.

Tested on 2026-09-27 on an isolated live server
([e2e/test_control_signals_only.sh](../e2e/test_control_signals_only.sh)). The
parent is durable and runs `WaitForSignal finance`, then `WaitForSignal legal`:

1. The parent parks on `finance`. Its only open request is `finance`.
2. `legal` is answered through `POST /api/runtime/signals/{instanceId}`. The
   request ID is computed in advance as `sha256` of legal's signal ID, which
   differs from finance's only by the step ID. The response is `404
   INPUT_NOT_FOUND`. Using the step ID `legal` as the request ID gives the
   same result. The parent stays parked on `finance`.
3. `finance` is answered (200). The parent then parks on `legal` with no
   answer: the request ID matches the computed one, so the early answer was
   aimed correctly and was dropped, not held.
4. `legal` is answered again, with the operation ID of the refused attempt.
   It is accepted, since a refused submission leaves no receipt. The parent
   completes with both answers.

What signals alone can and cannot do:

- **Can:** collect several answers when they arrive in the order the parent
  waits for them, with validation, replay by operation ID, and store-freeing
  parking while waiting.
- **Cannot:** accept an answer to a wait that has not opened yet. Core
  `submit_input` requires the request to be registered, and only the step that
  is waiting registers one. Nothing buffers a signal for a later step, and no
  raw signal path remains (`send_custom_signal` has no callers). An approval
  that finishes early must keep retrying until the parent reaches its step,
  which needs a retry loop and a way to know when to stop. A parent also cannot
  wait on "whichever finishes first", or on "all of them" in any order.

This is why the WaitForInstances step exists. It waits on the children's terminal
states, which are recorded on each child whenever it finishes. A child that
finished before the parent reached its wait counts at once, so arrival order
does not matter, and both `all` and `any` can be expressed.

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
- WaitForInstances rejects targets that are not the caller's children.
- A capability declaring `suspends: true` is rejected by validation on a
  non-durable step or one without a step timeout, and in unsupported contexts.
- A suspended caller resumes after a server restart; the agent receives the
  continuation state it returned and does not repeat work done before
  suspending. The step timeout fails the step even while it is parked, and the
  WaitForInstances deadline returns partial outcomes without failing.
- A suspended instance's compiled package survives artifact cleanup until the
  instance is terminal.
- A child that finished long ago stays readable through `get` and WaitForInstances while
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
