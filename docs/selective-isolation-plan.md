# Selective isolation implementation plan

Status: implementation plan, 2026-09-06. Baseline `0a04277c` in the audit
worktree. The [production research](isolated-step-production-research.md) and
[cancellation proof](isolated-step-cancellation-poc.md) establish feasibility and
some costs; neither implements this plan. No DSL or production emitter changes
are part of this planning deliverable.

## Non-negotiable compatibility contract

1. **No DSL changes.** No step variants, fields, annotations, defaults, mapping
   syntax, schemas or authoring migrations. In particular, no `isolated`,
   `async`, `await`, or cancellation-policy field. Selection is an internal
   compiler/runtime concern, recorded in compiled artifact metadata.
2. Every definition accepted by the baseline remains accepted. Retain validation
   and diagnostic behavior, including existing E128 Agent/Embed timeout rejection
   and E129/E130 restrictions. Do not repurpose legacy timeout fields in this
   change. Existing capability timeouts and AiAgent `turnTimeout` retain meaning.
3. With no new targeted cancellation command, preserve outputs, error envelopes,
   retries, invocation counts, checkpoints, wake sets, signals, debug behavior,
   connection binding and established ordering. Performance must also pass the
   rollout gates below; compiling successfully alone is insufficient.
4. Keep graph control in WASM. The host owns execution resources, I/O, atomic
   persistence and cancellation delivery. It does not interpret edges, select
   branches, evaluate expressions, retry steps or choose recovery handlers.
5. Keep old artifacts runnable and resumable with their original compiler/runtime
   contract. Do not recompile or switch a parked execution to the new layout.
6. Do not advertise independent hard cancellation for a boundary still using
   shared execution. Compatibility fallback is visible in artifact inspection
   and runtime capability reporting, not a silent promise of isolation.

“Preserve support” refers to today's accepted language, including accepted shapes
that execute sequentially. It does not imply enabling previously rejected forms.
Actual runtime equivalence of the future implementation must be proved by the
gates in this plan; it is not established merely by writing this document.

## Architecture and selection

Introduce an internal execution-boundary plan alongside the existing
`DirectRunPlan`, after validation, child resolution and graph ordering. Keep the
existing manifest's DSL semantics and checksum inputs intact; add a separate
artifact-layout version and compilation cache key. Use three internal boundary
kinds: local graph operation, capability invocation, and child graph invocation.
These names describe proposed internal types, not DSL constructs.

Selection is structural, not based on a guess about how expensive a step is:

- Keep Finish, Conditional, Switch, Split/While controllers, Log, Error, Filter,
  GroupBy, Delay and WaitForSignal in their owning workflow Store.
- Target Agent capability invocations for disposable Stores, including calls
  reached through AiAgent tools, providers, memory loading/saving/compaction and
  workflow-as-agent packages. A small `utils` Agent is still an Agent boundary;
  local Filter/GroupBy operations are the cheap operations retained in the parent.
- Compile each resolved EmbedWorkflow graph as a private child component invoked
  in a separate Store. The child runs the same graph compiler recursively; it
  must support every construct the inline child currently supports.
- Keep AiAgent conversation control in its owning WASM Store. Isolate its external
  capability calls and embedded tools; do not move the tool loop into Rust.
- Do not isolate each Split iteration or fan-out branch as an entire new workflow
  by default. Their existing WASM controllers start/join isolated invocations at
  the same scheduling points they currently invoke components.

The rollout selector is compiler/deployment configuration, outside workflow JSON.
Initially default it off, with explicit test modes for legacy and isolated
lowering. Enable by approved artifact capability and runtime ABI support. An old
runtime must continue receiving old-format artifacts; negotiate support before
compilation/registration, never silently reinterpret an artifact at execution.

### Component state is an eligibility question

Fresh Store per call resets mutable component globals and initialization state.
Today's composed instances may be reused between calls. Therefore independent
invocation equivalence is a required property of each admitted package, not a
property inferred from `Agent` syntax, a capability name or the two benchmarked
agents. Test repeated calls, alternating capabilities and initialization imports.

For first-party packages, audit and test the full set. For unknown/custom packages
without this guarantee, retain existing component lifetime and execution path;
do not reject the workflow or reset its state speculatively. The package's current
accepted use remains supported, with its current cancellation granularity.
An eventual stateful isolation contract would require explicit state transfer or
a longer-lived disposable region; it is separate from this per-invocation change.
If a product release requires independent cancellation for every Agent, this
eligibility gap is a release blocker, not permission to narrow Agent support.

## Compiler integration: replace the call, preserve its wrapper

| Location | Planned change and preservation rule |
|---|---|
| `direct_wasm/manifest.rs`, `plan.rs`, `child_workflows.rs` | Keep normalization, graph traversal, shared continuations and resolved child version closure. Add private boundary metadata without altering the graph. |
| `compile/agent_invoke.rs`, `agent_io.rs` | Add an invocation backend after current mapping/validation, connection resolution and workflow-agent scope wrapping. Return the same canonical success/error representation. |
| `compile/agent.rs`, `agent_retry.rs`, `agent_error.rs` | Keep checkpoint-hit short circuit, retry accounting, rate-limit handling, result shaping and onError in parent WASM. One host start is one capability attempt, never a host retry loop. |
| `compile/embed_workflow.rs`, `embed_retry.rs` | Replace only inline child execution with a child invocation. Retain call-site mapping, validation, variable scope, retry wrapper, result checkpoint, error wrapping and parent continuation. |
| `compile/ai_agent_loop.rs` | Cover all invoke sites: brain, tools, memory load/save and compaction. Preserve per-turn snapshots, tool counters, structured output and wait/embed tool behavior. |
| `compile/split_parallel.rs`, `branch_parallel.rs` | Replace direct async call sites too; changing `emit_agent_invoke` alone misses these. Keep launch/drain/assemble and wavefront scheduling semantics. |
| `compile/checkpoint.rs`, `loop_deadline.rs`, `abi.rs` | Preserve suspension propagation, deadline frames and outcome distinctions across child boundaries. |
| `component.rs`, `compile/artifact_metadata.rs`, `core_imports.rs` | Package dependencies once, wire the new generic imports and identify required runtime/layout versions. Preserve existing export modes. |

Source paths above are relative to
[the direct emitter](../crates/runtara-workflows/src/direct_wasm).
Every exhaustive `DirectRunPlan` match and component/import registry must be
reviewed when adding internal variants. Prefer a backend descriptor on an existing
invocation to duplicating entire retry or graph lowering implementations.

### Data crossing a Store boundary

Transfer owned canonical values, never linear-memory pointers, component resource
handles or private stdlib arena references. The current
[stdlib](../crates/runtara-workflow-stdlib/src/direct_json.rs) interns values at
16 KiB using run-local `$wfref`/`$wfnonce` handles. Materialize them in the sender
before crossing; re-intern only in the receiver's arena. Keep lookalike user
objects as ordinary user data. Initialize each child manifest independently.

Agent calls retain capability-id plus the existing JSON input bytes and
`error-info` ABI. `_connection` continues carrying the resolved opaque connection
identity; `connectionRef` wins over `connectionId` and is evaluated in the current
iteration/tool/child scope. No new raw credential channel or direct guest network
access. Runtime metadata and cancellation tokens are out of band and cannot be
overridden by workflow inputs.

Child-graph input contains the exact mapped data and computed variable scope,
logical invocation identity, effective durability mode, debug context and active
deadline context. Compute these using existing helpers before replacing the inline
call. Do not pass all parent locals by default or recalculate child scope from a
fresh root input. Child output is the existing call-site result; Finish completes
the child invocation, never the root workflow.

## Generic execution interface and ownership

Private, versioned execution imports:

```text
start(package-local artifact, entry, input, invocation context) → task resource
request-cancel(task) → requested | already-requested | already-terminal
join(task) → completed(bytes) | failed(error-info) | suspended(wakes)
             | cancelled | timed-out | trapped(detail)
release(task)
```

The initial [execution WIT](../crates/runtara-workflow-wit/wit/execution/runtara-workflow-execution.wit)
and host imports have been validated against the pinned component async implementation
using real parent-WASM control flow and built HTTP/utils agents. `join` and `release`
are awaitable from async-typed exports. Durable command identity and deduplication
belong to the execution-control coordinator below; the live task import itself
operates on an already resolved, owned resource. This ABI proof is not production
compiler/context wiring or persistent command handling.

Use resource ownership and invocation generations, not guest-chosen integer IDs
that can address another tenant's tasks. Invalid, stale and released handles have
defined errors; cancellation and release are idempotent at the command layer.
Cancellation acknowledgement is distinct from `join` confirming teardown.
Keep descendant teardown outside the cancellable execution future: the task
supervisor must await it even after that future is dropped or panics. A cleanup
failure (`worker-lost` at the execution interface) is a host failure requiring
root fencing/termination, not a recoverable guest trap or an ordinary step retry.
The scoped task primitive and resource adapter now have tests for this ordering;
production scope construction and durable fencing are still required. The
context-enabled root entry now also stages host-runtime `complete`/`fail` calls
until descendant cleanup succeeds, with tests observing the callback boundary
rather than only the final returned result. Final publication retains the
original timeout and cancellation guards; persistent commit fencing remains
necessary.

Prepare/cache code before admission; install CPU epoch and pending-I/O guards
before instantiation. Store destruction and descendant reaping precede publishing
a cancelled/trapped result to the parent. Retain the parent while it is joining.
Cancellation during queued admission allocates no Store and launches no I/O.
Task resource drop schedules owned cleanup; the executor must await that cleanup
before considering the root run released. Never detach children indefinitely.

Parent WASM maps outcomes into its existing error/recovery machinery. A suspension
is not an error, retry, completion, or empty output. Existing errors preserve
their codes/categories/retryability. New targeted cancellation uses an internal
non-retryable cancellation reason; it must not accidentally consume Agent's
default retry policy and restart cancelled work. Explicit onError recovery can
run after the child is reaped. A whole-root cancellation bypasses local recovery
and tears down the tree according to root lifecycle semantics.

Task groups can be generic ownership resources for an active Split, While,
EmbedWorkflow or AiAgent invocation. Parent WASM registers descendants and decides
which group to stop. This permits stopping an active operation owned by a compound
step without pretending that every local instruction has a separate Store.
Preempting a non-cooperative local Filter/GroupBy operation while preserving that
same Store remains outside the hard-cancellation guarantee.

### User command routing without a DSL extension

Add an execution-control command, outside workflow definitions, addressing the
root instance, logical invocation path and active attempt generation. The UI
obtains that opaque address from runtime step activity; a bare step ID is
ambiguous in loops, nested children and repeated AI tool calls. Scope authorization
to the tenant/root and retain the command ID for deduplication. Do not overload
a workflow's custom signal name or consume its WaitForSignal payload as a cancel.

At an isolated wait point, generated parent WASM awaits both task completion and
execution-control notification. If cancellation is selected, it identifies the
owned task/group, requests cancellation, joins teardown and routes the typed
outcome. This selection is generated control flow, not a Rust graph scheduler.
The underlying notification, task wakeup and atomic race arbitration are generic
host services. Ancestor cancellation propagates to descendants, while an exact
attempt command cannot cancel a later retry or a sibling with the same step ID.

| Command target/state | Required result |
|---|---|
| Active isolated invocation | Record the command, fence the target attempt, stop/join it, then allow the owning WASM handler to continue. |
| Queued invocation | Cancel admission without constructing a Store or issuing I/O. |
| Completed invocation / stale generation | Return terminal/no-op status; never reinterpret it as cancellation of the current attempt. |
| Parked invocation | Persist targeted intent at its logical address; on root replay, consume it before restarting that invocation. Retain unrelated wake registrations. |
| Local or legacy shared boundary | Report its actual cancellation capability; preserve execution rather than killing the root to imitate step cancellation. |
| Root Stop / pause / shutdown | Preserve current command distinctions and acknowledgement semantics; coordinate the entire tree, with pause/drain using existing checkpoint/settlement rules. |

Do not silently make an existing root Stop button cancel only the currently
visible step. New targeted UI/API control is additive; root control keeps its
identity. Hard root shutdown may require native enforcement even if parent WASM
cannot process a notification, as it does for whole-execution limits. That does
not grant the host authority to choose a recovery edge.

## Child runtime and durable suspension

The [runtime interface](../crates/runtara-workflow-wit/wit/runtime/runtara-workflow-runtime.wit)
includes root lifecycle operations. Passing an unrestricted root `RuntimeHost` to
a child would allow it to complete/fail or suspend the root prematurely. Introduce
a child-scoped adapter with explicit behavior for **every** runtime method:

| Method family | Child adapter behavior |
|---|---|
| `load-input`, `instance-id` | Child input envelope; retain the public root instance identity and carry invocation identity separately. |
| `complete`, `fail` | Capture child terminal outcome; never mutate root terminal status. |
| `get-checkpoint`, `checkpoint`, `record-retry-attempt` | Preserve logical keys and payloads; authorize/fence writes for this attempt. Avoid adding the namespace twice. |
| `poll-custom-signal` | Use the existing derived checkpoint address; preserve signal identity, consumption and replay behavior. |
| `is-cancelled`, `check-signals`, `handle-checkpoint-signal` | Consult a root-owned command coordinator; propagate typed suspension/cancellation and acknowledge each root command once. |
| `breakpoint-pause`, `debug-mode-enabled` | Preserve child step path and pause-before-side-effect semantics; root performs the lifecycle transition. |
| `custom-event`, `heartbeat` | Forward with authorized logical scope; preserve existing event payloads and identity. Coalescing heartbeats must not hide liveness. |
| `durable-sleep`, `blocking-sleep`, `durable-sleep-checkpoint`, `now-ms` | Preserve the selected entry ABI's blocking/parking behavior, absolute deadlines, clock source and saved wake checkpoints. |

Some sleep functions are linker glue rather than `RuntimeHost` trait methods;
audit both the trait and WIT bindings. A fresh composed SDK runtime must not
register an independent root execution for each child. Production isolated child
graphs use the scoped host binding; legacy composed runtime behavior remains on
the legacy backend for its supported reference/migration uses.

The native child adapter and prepared-launch scope factory now implement explicit
checkpoint authority, local terminal validation, inherited absolute deadlines and
root-owned deferred command receipts. The supervised root entry now shares those
receipts with the parent runtime and finalizes them after cleanup, before terminal
publication. Real PostgreSQL tests include prepared WASM roots/children and
preserve the legacy runtime checks. Production runner integration,
compiler namespace authorization, persistent attempt fencing and parking/wake
qualification are still pending; see the
[implementation record](selective-isolation-implementation.md#supervised-root-lifecycle-coordination).

Scoped parent and child lifecycle checks now share the root's existing poll
interval and a single in-flight read. Cached positive results remain visible to
all siblings; explicit checkpoint receipt validation remains fresh. Trusted
checkpoint/sleep responses invalidate cached absence without blocking on another
poll. Cancellation of a polling child releases the read for its peers. These
limits cover ordinary lifecycle polls, not checkpoint IO, custom-signal reads or
heartbeat coalescing; production capacity qualification must measure all of them.

Suspending a child returns the lifecycle **wake set**, including absolute timed
wakes, signal addresses/deadlines and on-resume. Parent WASM follows the existing
sibling settle/checkpoint rules, then propagates the required wake set to its
caller. The host must not independently relaunch the child as a workflow. On
relaunch, the pinned parent artifact replays and invokes the child at the same
logical address; completed work is recovered from existing checkpoints.

Preserve loop deadline state and completed-loop markers across the boundary.
Transfer the effective enclosing deadline into the child context and restore the
parent scope on return/error. Do not restart a relative timer after replay, extend
the budget on retry, or leave an expired inner deadline active during recovery.
Do not turn existing cooperative deadline checks into new preemption behavior in
the parity release. Hard enforcement at these same fields is a separately gated
behavior expansion with explicit error/side-effect tests, not a DSL modification.

### Fencing without changing existing durability settings

Keep logical checkpoint keys stable, including iteration addresses, embed call
sites and AiAgent per-tool-call counters. Add attempt generation and launch lease
to authorization/context and a separate attempt ledger; do not append generations
to every existing key and thereby lose successful replay hits.

Use conditional durable transitions to arbitrate cancellation and completion.
Checkpoint upsert alone is insufficient. A cancellation winner fences subsequent
writes and completion from that attempt; a committed completion wins over late
cancel. Stop old owners or revoke their lease before reclaiming resources on a
new worker. Transactional fencing cannot undo an external side effect already
issued; retain current idempotency expectations.

Workflow and step `durable:false` must retain their scope and replay behavior.
Do not introduce result memoization, hidden per-step replay checkpoints or a
mandatory database round trip for every non-durable invocation. Such calls use
in-memory attempt arbitration during a live run; any persisted user cancellation
tombstone prevents that cancelled invocation from restarting but is not a success
cache. Internal steps of an Embed or Split do not inherit a call-site-only
durability override that previously did not apply to them.

## Complete DSL construct matrix

This table covers all 14 variants in the current
[Step enum](../crates/runtara-dsl/src/schema_types.rs). Every row applies at root,
inside Split/While, inside EmbedWorkflow and in any currently permitted onError,
onWait or AiAgent tool position; the matrix does not expand legal placements.

| Construct | Placement and mandatory compatibility cases |
|---|---|
| Finish | Local to owner; child Finish returns to caller, early Finish terminates its region, absent Finish retains implicit null behavior, output mapping and schema errors unchanged. |
| Agent | Isolated eligible call only; mappings, validation, connection refs, defaults, retries/rate limits, checkpoint hit/miss, errors, durable override, breakpoint, workflow-agent scope and custom-package fallback. |
| Conditional | Local; boolean output, true/false labels, nested diamonds, shared merges and condition failures unchanged. |
| Split | Local controller; input-order results, empty arrays, variables/index, schemas, nested bodies, dontStopOnFailed aggregation, timeout/retry budgets, per-item keys and durability scope. Existing parallel eligibility and sequential fallback remain. |
| Switch | Local; value and route forms, default/missing/default-value behavior, coercions, output envelopes and branch merges unchanged. |
| EmbedWorkflow | Separate eligible child graph; resolved version closure, schemas, variables, parent references after return, local onError, nested retries, repeated call sites, suspension and mixed nested deadlines. |
| While | Local controller; max iterations, condition timing, loop index/previous outputs, variables, timeout persistence and nested error unwind unchanged. |
| Log | Local; levels, event payloads, failure attribution, tracking disabled/enabled and position among parallel siblings unchanged. |
| Error | Local; structured code/category/context, handler selection, termination scope and debug error event unchanged. |
| Filter | Local; all accepted operators, missing/null/type behavior and mapping semantics unchanged; large outputs materialized if later sent to a child. |
| GroupBy | Local; key resolution, grouping/output shape, ordering where specified, missing keys and large groups unchanged. |
| Delay | Local durable controller; dynamic duration, checkpoint address, early relaunch/repark, enclosing deadline clamping, CLI blocking reference mode and current non-durable rejection unchanged. |
| WaitForSignal | Local durable controller; scoped signal keys, payload mapping, consumption, deadline/skew rules, onWait nested graphs/errors, retries/polls and wake propagation unchanged. |
| AiAgent | Local controller with isolated external invocations; single shot/tool loop, Agent/Embed/Wait tools, dynamic provider connection, retries/defaults, turnTimeout transport semantics, structured output, memory/compaction, per-turn replay, counters and breakpoints unchanged. |

Cross-cutting: test normal/conditioned/error/tool/memory edges; unconditional DAGs
with cross-branch references; merges executed once; disconnected/unreachable
validation behavior; literal/reference/template mappings; missing/null and numeric
coercion/defaults; quoted dotted keys; top-level and nested schemas; debug path and
step identity. Do not treat an existing parallel fallback as unsupported syntax.

Workflow-as-agent publication safety checks remain intact. The low-level
runtime-importing AgentCapabilities mode used for compatibility tests does not
authorize publishing new suspending workflow agents. Preserve both that reference
mode and today's certified package support; do not apply the publication safety
subset to all EmbedWorkflow graphs.

## Parallelism and resource admission

Keep the existing `parallel_agent_body` predicate initially, including fallback
for retrying items, workflow-agent bodies, breakpoints and Split-level timeout or
retry wrappers. Its implementation is authoritative; the file header describes
an older, narrower stage. Preserve the current pool/window cap and input-order
assembly. Do not expand concurrent side effects simply because Stores permit it.

Existing branch wavefront execution must still start eligible independent work
before settling it. Preserve producer-before-consumer dependencies, sibling
settlement before parking, and replay that does not double-fire completed peers.
With isolation enabled, all existing HTTP overlap tests must remain actual overlap
tests, not be replaced with output-only assertions.

Reserve per-root and per-tenant capacity for runnable descendants. Do not use one
permit pool in which waiting roots hold every child permit. Account for aggregate
memories (not only `memory_peak_bytes`), tables, host buffers, handles, queued work
and compiled code. Release reservations after teardown. Existing accepted payload
and nesting limits must not shrink accidentally due to new defaults. Where extra
copies increase memory cost, meet the previous workload envelope or keep the
feature off for that workload until capacity is provisioned and measured.

Operational admission failures are typed resource failures, not DSL validation
errors. Cancellation must have a control path and cleanup capacity even when work
queues are full. Avoid holding Store locks while awaiting child admission.

## Packaging, entry modes, and deployment

Package the statically resolved dependency closure once per digest, using
package-local references. Preserve resolved `latest` versions across replay.
Cache compiled definitions with engine/configuration/ABI/layout identity; bound
cache bytes separately from guest memory. Run compilation in the existing
killable prepared-worker path, outside the invocation cancellation path.

Do not duplicate a full dependency tree at each nested embed. Child references
resolve through the immutable parent package catalog. Keep only actual
dependencies. Package parsing must bound total expanded size, nesting and counts,
verify hashes and ABI compatibility, and reject invalid compiled artifacts before
registration. This is artifact validation, not a new restriction on valid DSL.

Logical Agent lowering now packages a deterministic call-site inventory from the
same normalized compiler manifest as the emitted code. Raw package version 2 and
native envelope `RTRNP002` carry it through worker verification, preparation and
cache/queue ownership. Both decoders reject missing or inconsistent versioned
metadata. Existing raw/native formats remain readable; legacy compilation still
uses its original format. The sidecar reports `packageVersion` for inspection,
but execution must use the inventory bound to the verified package.

This inventory identifies workflow, binding, Agent, capability, step and allowed
AI invocation domains. The current adapter uses canonical v3 paths and call-site
tokens, with structured child/loop frames and valid counters; historical path
versions retain their versioned decoders. The prepared launcher validates static
identity and compiler-defined namespace/loop membership before the scope factory
grants checkpoint authority. Auxiliary AI calls retain their compiler-defined
attempt/activation convention.

Inventory v5 additionally records effective durability for every emitted caller,
including AI tool and auxiliary calls. The value comes from the normalized
compiler definition, never from runtime input or the target tool's step setting.
Missing, extra or duplicate durability entries are rejected. Runtime admission
retains inventory v4 support with unknown durability; unknown and explicit false
cannot authorize durable fencing. New compilation uses a distinct cache provenance
tag, without changing the guest ABI or raw/native envelope versions. Ordinary
execution still creates no invocation ledger entries. Production initial admission,
root lease ownership, root/parent write fencing and targeted command routing remain
required before durable cancellation can be enabled.

| Existing entry/runtime mode | Compatibility policy |
|---|---|
| InvokeHostImports + HostImport (production) | Primary isolated backend and complete parity gate. |
| CliRunHttp + HostImport (reference) | Keep original compiler/runtime path and tests; no new mandatory execution imports. |
| Composed runtime / wasmtime CLI reference | Keep runnable legacy artifacts and differential coverage. Standalone CLI cannot satisfy new execution imports; never silently emit those into this mode. |
| AgentCapabilities | Retain certified non-suspending publication and existing low-level migration/test mode. New isolated package requirements must be understood by the consumer before export is selected. |
| Pure workflow omitting runtime | Retain omission and no-host execution when no isolated boundary is needed. Do not add imports to every workflow unconditionally. |

This preserves current deployment choices while adding an execution mechanism to
the compatible production host. If cross-runtime execution of the *new isolated
artifact* is required, that host must implement the generic imports; this is a
runtime dependency, not a workflow-schema change.

## Test plan and merge gates

Use the existing full execution harness in
[direct_wasm_execute.rs](../crates/runtara-workflows/tests/direct_wasm_execute.rs),
not a reduced new suite. Add an explicit backend parameter that actually routes
every eligible invocation through the isolated executor. Assert observed starts,
artifact identity, teardown and backend coverage so a passing test cannot silently
exercise legacy fallback. Keep deliberately ineligible packages covered as such.

For each supported fixture run identical inputs, seeded/mock nondeterminism,
provider response scripts, clock/signal schedules and preloaded checkpoints through
legacy and isolated production modes. Compare result/error JSON, request payloads
and counts, retry traces, checkpoint keys/state, wakes and event partial ordering.
Normalize timestamps, random values and internal handles only where not part of
the public contract; do not normalize away error fields, missing events or calls.
Compare graph-required happens-before relationships, not an invented total order
between independently concurrent requests.

| Gate | Existing anchors (test names) | New required cases |
|---|---|---|
| G1 Language/validation | Entire DSL/workflows native suites and 30 native audit tests | Accepted/rejected corpus identical in both modes; serialized schemas/defaults unchanged; exhaustive 14-variant boundary inventory. |
| G2 Basic output/data | `direct_wasm_execute_fanout_cross_branch_reference_runs_producer_first`, `direct_wasm_execute_named_key_into_split_array_output_fails_loud` | Cross-Store payloads at 16 KiB−1/16 KiB/16 KiB+1 and MiB scale; nested arena refs, handle-lookalike user objects, error bytes, no child memory aliasing. |
| G3 Durability/retry | `direct_wasm_execute_durable_agent_retry_per_iteration_isolation_across_resume`, `direct_wasm_execute_invoke_embed_workflow_retry_parks_before_second_attempt` | Kill/relaunch at every invocation and checkpoint boundary; cancelled generation rejected; cache hits perform zero starts; durable:false preserves invocation counts. |
| G4 Suspension/signals | `embedded_children_waiting_on_same_step_get_per_site_signal_ids`, `scoped_signal_wait_survives_drain_and_resume`, `pause_inside_nested_composed_agents_chains_the_suspend` | Child suspended is never retry/error; root command acknowledged once; multiple sibling wakes retained; child cannot complete/fail root. |
| G5 Time budgets | All 47 executed emitter audit cases, including `audit_05_child_timeout_unwind_restores_the_parent_budget` | Same deadline/checkpoint bytes through isolated Embed; early relaunch, overflow/zero, completed-loop replay, inner handler budget restoration. |
| G6 Parallel semantics | `direct_wasm_execute_parallel_split_http_overlap`, `direct_wasm_execute_parallel_branches_http_overlap`, `direct_wasm_execute_parallel_branches_durable_resume_no_double_fire`, `direct_wasm_execute_invoke_parallel_split_item_retry_parks_sequentially` | Cancel one active peer, retain siblings; unchanged window and assembly order; saturated queues; cancel while queued; no fallback merely because isolation is selected. |
| G7 AI/connection/debug | `direct_wasm_execute_ai_agent_resolves_connection_ref_and_runtime_model_parameters`, `direct_wasm_execute_ai_agent_loop_non_durable_skips_turn_checkpoints`, `direct_wasm_execute_ai_agent_memory_emits_debug_events` | Every provider/tool/memory path observed isolated; nested tool scope and connection precedence; cancel during brain/tool/compaction; no extra billing calls on replay. |
| G8 Host lifecycle | All ten existing live cancellation proofs | Pre-start, initialization, CPU, header/body wait, completion race, duplicate/stale commands, descendant cleanup, cancellation under admission and result-buffer pressure. |
| G9 Artifact/package | `direct_wasm_execute_invoke_omit_runtime_pure_workflow_runs_with_no_runtime_host`, `direct_wasm_execute_agent_capabilities_workflow_invocable_as_agent` | Legacy artifacts and parked runs unchanged; missing ABI detected early; dedup repeated nested children; malformed package; cache eviction; unsafe native cache rejected. |
| G10 Capacity/state | Existing large-scope heap/event tests and research tests | Repeated stateful capability calls, initializer count, memory and task leaks over long runs, first-party package eligibility, no cross-tenant handles, Linux load/cancel p99 measurements. |

Inventory all tests actually compiled/executed under each feature combination;
test count equality alone is insufficient. Keep assertions, fixtures and timeouts
at least as strong as the baseline. A skip, ignored test, new validation rejection,
forced sequentialization of existing overlap, or unexpectedly unavailable backend
fails the gate unless it was already required by that reference mode.

Required commands as the affected boundary grows:

```sh
cargo fmt --all -- --check
RUSTC_WRAPPER= cargo test -p runtara-dsl
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests
RUSTC_WRAPPER= cargo test -p runtara-component-host --features component-integration-tests,isolated-step-poc
RUSTC_WRAPPER= cargo clippy -p runtara-component-host --all-targets --features component-integration-tests,isolated-step-poc -- -D warnings
```

Run those workflow tests in both new backend modes, and preserve the existing
`RUNTARA_DIRECT_RUNTIME_BINDING=composed`, `RUNTARA_DIRECT_WORKFLOW_ABI=cli-run`
and `RUNTARA_DIRECT_WASM_EXECUTOR=cli` reference axes where applicable. The backend
selector itself is not implemented yet. WIT/guest changes also require
`scripts/build-agent-components.sh`, owning runtime/stdlib tests, workflow Clippy
and the feature-gated CI matrix. Persistence changes require core and store
integration tests against isolated databases; server/environment changes require
their feature-gated lifecycle suites. Derive exact feature/service setup from
[CI](../.github/workflows/ci.yml), including SQLX_OFFLINE for applicable builds.

## Implementation sequence and completion criteria

| Phase | Deliverable | Exit criterion |
|---|---|---|
| P0 Baseline and differential harness | Freeze fixture/schema/diagnostic and execution traces; expose backend selection in tests | G1 inventory, reference suite green, no production default changes. |
| P1 Generic executor and ABI | Bounded task ownership, prepared code, guards before init, tagged outcomes, data transfer | G2/G8 host tests; no DSL or graph scheduling in host. |
| P2 Agent backend | Replace sequential and parallel call boundaries, include AI auxiliary calls | G2/G3/G6/G7 pass; actual isolation observed; package-state eligibility recorded. |
| P3 Child graph/runtime adapter | Extract embeds recursively, scoped lifecycle and complete wake propagation | All 14 constructs in nested contexts; G3/G4/G5/G9 pass, no new child safety subset. |
| P4 Durable cancellation integration | Conditional attempt transitions, root command coordination, targeted command routing | Crash/race/lease and signal tests pass with real persistence; legacy Stop semantics retained. |
| P5 Production qualification | Package trust/cache, quotas, deployment compatibility, Linux benchmarks | G1–G10 pass; old parked executions replay; capacity envelope demonstrated before enabling. |
| P6 Controlled enablement | Enable approved compilation/runtime combinations; monitor outcome, latency and memory regressions | Rollback stops creating new isolated artifacts while retaining execution support for existing ones. |

P1–P3 may be developed behind flags, but an Agent-only proof is not completion of
this plan. Do not declare rollout-ready until nested Embed, Wait/Delay, AI tools,
parallel execution and durability have passed the same compatibility contract.
Rollback cannot remove the new runtime while new-format executions remain active
or parked. Do not shadow-run real side effects for comparisons; use recorded
fixtures or deterministic service doubles, then canary separate real executions.

There is no promised performance threshold yet. Before P5, set deployment-specific
latency and memory budgets from Linux measurements; require no regression in
existing overlap and bounded-memory tests immediately. If the measured envelope
does not fit, improve allocation/caching/copying or provision capacity before
enabling, rather than lowering accepted DSL limits.

## Measured baseline and isolation comparison

The [workflow baseline report](research/workflow-performance-baseline.md) now
records **11 real emitted workflow cases**, measured twice in release mode on an
Apple M3 Max (16 cores, 64 GiB). The
[machine-readable measurements](research/workflow-performance-baseline.json)
include exact graph definitions, input/dependency/artifact hashes, sizes and
timing summaries. These are historical legacy-only results. The current
Agent-only candidate has a separate paired comparison below; use that comparison's
same-revision legacy measurements when calculating overhead.

| Representative case | Composed WASM | Cached full execution p50 | JSON → first result p50 |
|---|---:|---:|---:|
| Finish only, small payload | 2,803,799 bytes | 0.081–0.088 ms | 186.6–191.2 ms |
| One random-double Agent + Finish, DSL defaults | 3,308,070 bytes | 0.143–0.148 ms | 230.1–231.5 ms |
| Ten random calls in a chain | 3,324,803 bytes | 0.454–0.490 ms | 224.3–251.4 ms |
| 100 random calls in a chain | 3,515,648 bytes | 19.855–20.129 ms | 255.3–268.2 ms |
| Sequential Split, 100 random calls | 3,310,703 bytes | 9.782–10.499 ms | 229.4–244.5 ms |
| Parallel Split, window 4, 100 random calls | 3,318,568 bytes | 12.454–12.758 ms | 243.8–245.1 ms |
| Embedded child with one random call | 3,309,475 bytes | 0.185–0.207 ms | 219.3–227.9 ms |
| Finish returning a 1 MiB input value | 2,803,823 bytes | 4.250–4.556 ms | 192.2–199.3 ms |

Ranges are the two runs' medians, not confidence intervals. All random outputs
are checked for numeric type, range and expected count. The durable cases also
check that replay returns byte-identical cached random output. The parallel case
asserts that the compiler actually composed a four-instance pool.

For the default single random workflow, only **60,967 bytes** are workflow logic;
the complete artifact is **3,308,070 bytes**, or **848,296 bytes gzip**. Its
serialized native component is **12,456,600 bytes**. DSL-to-WASM takes roughly
32–33 ms, and native compilation roughly 196–197 ms. Thus the cost of a prepared
execution and a first execution starting from DSL differ by three orders of
magnitude; these must be separate comparison metrics.

“Cached full execution” includes a fresh Store, WASI setup, instantiation, the
entire graph, host runtime calls, result handling and Store teardown through the
production `WorkflowExecutor`. It excludes compilation and preparation. “JSON →
first result” includes parsing/emission, composition/artifact I/O, uncached native
compilation, prepared linking and execution, with an already-created engine and
possibly warm filesystem pages. Engine/executor construction is recorded
separately. Neither figure includes Cargo compilation or server-side validation,
API requests, authentication, image registration/download, queues or a real
database. The capturing runtime host stores checkpoints and events in memory.

Comparison conclusions limited to this baseline:

- The 100-step chain adds about 205 KiB over the non-durable single-call artifact, not 100
  copies of the utils component. Preserve dependency reuse when adding isolation.
- The parallel random Split is slower than its sequential counterpart on this
  workload. Tiny CPU operations provide no network-wait overlap to offset the
  scheduling machinery; do not promise universal speedups from concurrency.
- The chain and Split have different state/scoping and output collection paths.
  Their timings are not a controlled comparison of one emitter instruction;
  preserve both as workload tests instead of multiplying single-call latency.
- Durability and event tracking are separate cases. In-memory checkpoint timing
  must not stand in for deployed persistence latency, and larger payloads need
  separate tests for materialization/copying costs.

### Current Agent-only paired comparison

The [paired report](research/workflow-performance-comparison.md) and
[raw samples](research/workflow-performance-comparison.json) record three release
sessions at implementation `fe014566d3ee8d28e0f3e5bf3f26f7610bb868ba` on the same M3
Max. Backend order alternates. Both sides use identical workloads and dependency
hashes, and the same bounded worker preparation path with disk caching disabled.
The exact benchmark executable hash is retained and checked across sessions.

| Measurement | Matched legacy | Isolated Agent adapter | Paired change |
|---|---:|---:|---:|
| Default random-double + Finish, complete WASM | 3,308,198 bytes | 3,314,102 bytes | +5,904 bytes / +0.18% |
| Same workflow, complete gzip | 848,314 bytes | 849,699 bytes | +1,385 bytes / +0.16% |
| Same workflow, complete native worker payload | 12,456,600 bytes | 12,532,110 bytes | +75,510 bytes / +0.61% |
| Same workflow, prepared full-run p50 | 0.155–0.162 ms | 0.215–0.227 ms | +33–47% |
| Ten random calls, prepared full-run p50 | 0.474–0.487 ms | 1.041–1.127 ms | +117–131% |
| 100 random calls, prepared full-run p50 | 20.104–20.612 ms | 26.246–27.056 ms | +31% |
| Parallel Split, 100 calls/window 4, full-run p50 | 12.267–12.782 ms | 15.120–16.045 ms | +20–26% |
| Default single random workflow, DSL → first result p50 | 284.119–299.156 ms | 301.488–323.097 ms | +3–8% |

Ranges are per-session medians, not confidence intervals; percentage changes use
paired sessions. Binary growth is small in these cases, but execution overhead is
material for cheap CPU-only Agent calls. The ten-call graph more than doubles its
prepared latency. Do not translate these total-run differences into exact
Agent-only service time or predict HTTP/AI behavior without separate measurements.

Each isolated random case contains one unique packaged utils component, even for
100 calls or parallel pools. Separate instrumented runs assert real child launches,
released task results and zero launches on byte-identical checkpoint replay.
Headline warm runs omit the optional launch counter; the report shows the
instrumentation comparison separately. No failures are excluded from the samples.

This candidate isolates Agent calls only. Embed remains inline, and Finish-only
workloads run the legacy executor on both sides as controls. The cold preparation
phases have only three samples per session; even the unchanged Finish controls
show substantial cold-time variation. These are exploratory results, not a
production acceptance gate. Direct step spans, aggregate memory/RSS, server
persistence, cancellation responsiveness and Linux load/tail qualification remain
required. Earlier historical cold timings use a different preparation boundary
and must not be substituted for the matched legacy column above.

### Measurement contract for baseline versus candidate

Keep the existing 11 cases as the minimum comparison suite. Run each unchanged
through the legacy and isolated backends on the same machine and revision, using
an explicit backend selector once available. Record actual isolated invocation
counts and fallback reasons alongside every sample group. A case that falls back
to legacy execution measures compatibility, not isolation overhead.

The single-step reference is one `utils/random-double` Agent followed by Finish,
with the generated value returned and checked. Keep separate cases for DSL
defaults, `durable: false`, durable execution and event tracking. Use the exact
same input and configuration for each backend; random output bytes need not match
across fresh runs, but type, range, count and replay behavior must match.

| Metric | Start → end / accounting boundary | Comparison purpose |
|---|---|---|
| Complete raw `.wasm` bytes | Entire distributable artifact, including the child catalog and embedded component bodies | Deployment/storage cost; a smaller root alone is not a size improvement. |
| Compressed package bytes | Compress the complete artifact with the same tool, version and flags | Transfer cost; do not sum separately compressed children as if that were the shipped package. |
| Root, logic and child bytes | Report root logic, root runtime/dependencies, catalog metadata, unique child bytes, unique child count and binding count separately | Explain fixed costs and verify that 100 calls do not embed 100 identical children. Identify overlapping size categories rather than summing them. |
| Prepared native bytes | Serialized root plus all unique prepared children and their index | Native cache/disk footprint, separately from portable WASM. |
| DSL compilation | Parse/validate/emit, compose/package, then total DSL → artifact | Compiler overhead, independent of Rust/Cargo dependency builds. |
| Native preparation | Artifact verification, native compile, deserialize/link and total preparation, with cache state labeled | First-use cost and prepared-cache benefit. |
| Single-step service time | Immediately before invoking the Agent boundary → result available to the parent | Invocation cost, including child admission, instantiation, input/output transfer and reaping when those occur inside this boundary. Report internal phases separately where available. |
| Parent step time | Before input mapping → output mapping/checkpoint/event handling complete | User-visible step cost, including orchestration inside WASM. |
| Prepared full execution | Fresh Store setup → graph completion, result collection and teardown | Existing cached full-run metric; includes Agent + Finish, runtime calls and all owned child cleanup. |
| First result from DSL | DSL input → first completed result, engine already created | Existing compilation + preparation + execution metric; explicitly excludes process startup and API queueing. |
| Server end-to-end time | Accepted execution request → persisted terminal result | Production overhead, with queue/admission, preparation, active execution and persistence spans. Measure client-observed request/response latency separately. |
| Cancellation and recovery | Cancel accepted → child stopped/reaped → parent recovery output | Responsiveness and preservation of the rest of the workflow; record all three timestamps. |
| Resource cost | Peak aggregate guest memory, process RSS, retained results/cache bytes, CPU time, active children and released resources | Capacity consequences that elapsed time and largest-single-memory telemetry miss. |

The current harness measures prepared full execution and first result from DSL;
it does **not** yet measure the two step spans or all preparation subphases above.
Add those spans to both backends before making claims about step-only overhead.
Use separate instrumented runs to attribute phases, and uninstrumented runs for
headline latency; quantify instrumentation overhead. Do not subtract Finish-only
latency from a full run and call the difference an exact Agent measurement.

### Workload and statistical comparison requirements

- Keep one, ten and 100 random calls, sequential and parallel Split, an embedded
  child, Finish-only and the large-output case. Report both total run latency and
  completed calls/second; dividing parallel elapsed time by calls is not individual
  step latency.
- Add child-boundary payload sweeps at 1 KiB, 16 KiB, 64 KiB and 1 MiB, including
  values immediately below/at/above the configured materialization threshold.
  The existing Finish-only 1 MiB case does not exercise a cross-Store transfer.
- Add controlled CPU work and delayed local HTTP, with success, retry, suspension,
  cancellation and sibling-continuation cases. Label unsupported baseline operations
  as unavailable rather than assigning zero time or simulating successful support.
- Measure durable first execution and checkpoint replay separately. For server runs,
  use isolated persistence and deterministic local services; record database and
  service configuration and distinguish service waiting from active CPU time.
- For invocation fencing, record database round trips, admission/settlement time,
  root-row lock wait and transaction duration separately from Store execution.
  Compare checkpoint, retry, event-heavy and sleep workloads, with parallel children
  under one root versus the same concurrency spread across independent roots.
  Report attempt-ledger rows/bytes per run and after retention. Verify ordinary
  non-durable calls introduce zero invocation-ledger IO; do not average that fast
  path together with durable arbitration when reporting single-step overhead.
- Run release builds with identical dependency hashes, engine settings, memory
  limits, event/durability options and inputs. Record OS/architecture, CPU/RAM,
  toolchain, revisions, artifact hashes, concurrency and warmup/sample counts.
  Label fresh process, cold native cache, warm prepared cache and replay explicitly;
  an uncached native compile does not imply a cold filesystem cache.
- Alternate backend order across repeated paired runs, run them serially without
  competing builds, and retain raw samples for the candidate qualification suite.
  Report median/p95 and absolute plus percentage differences; use enough samples
  for tail estimates. The existing 30/100-sample groups are exploratory evidence,
  not a reliable p99 gate. Use at least 1,000 completed samples per condition for
  production tail analysis and report uncertainty across independent runs.
- Report throughput and tails at concurrency 1/4/16/64 with the same offered load
  and resource limits. Include failures, admission rejections and cancellation
  counts; do not improve reported latency by silently excluding failed requests.

Publish one comparison row per workload, cache mode and metric:

| Workload / mode | Metric and unit | Baseline | Isolated | Absolute delta | Delta % | Samples / uncertainty | Correctness / isolation coverage |
|---|---|---|---|---|---|---|---|
| `random_1_defaults` / prepared | Full execution p50, µs | Same-session measurement | Pending backend | Pending | Pending | Per-backend counts and repeated-run spread | Output valid; actual child count verified |

Calculate percentage delta as `100 × (isolated − baseline) / baseline`; use N/A
when the baseline is zero or unavailable. Lower is better for bytes, latency and
resource usage; higher is better for throughput. Establish and record acceptance
budgets for package growth, latency, memory, throughput and cancellation before
candidate qualification, based on deployment requirements and the paired Linux
baseline. The local macOS medians are not those budgets. Any proposed default
rollout must include the completed comparison and explanations of regressions,
as well as passing compatibility gates.

### Measurement checkpoints in the implementation plan

Performance evidence is a deliverable at each affected phase, with the final
comparison required before enablement:

| Checkpoint | Required evidence |
|---|---|
| P0, before switching execution paths | Preserve the current measured baseline and fixture hashes. Add direct single-step spans and capture a fresh legacy run on the qualification host. Record missing metrics explicitly. |
| P1, executor and package foundations | Measure complete package/native-cache size, child creation and teardown, transfer costs and cancellation latency. These component measurements explain costs but cannot replace emitted-workflow measurements. |
| P2, first emitted isolated Agent | Run the exact single random-double + Finish workflow through both backends. Publish raw/gzip/native bytes, Agent service time, parent step time, prepared full-run time and cold first-result time, with absolute and percentage deltas. |
| P3, nested and parallel workflows | Repeat the comparison for chains, Split, nested Embed and payload sweeps. Verify dependency deduplication, actual isolated call counts, overlap and aggregate memory. |
| P4/P5, persistence and local server | Compare first execution, replay, retries and cancellation using isolated persistence and controlled services. Include submission-to-persisted-result latency, throughput and tail latency under load. |
| P6, enablement decision | Publish the paired report, raw samples, agreed budgets and pass/fail results. Explain regressions and retain the legacy default until qualification passes. |

For the single random-double reference, the current evidence and remaining work
must stay visible together:

| Measurement | Recorded legacy baseline | Remaining comparison work |
|---|---|---|
| Complete `.wasm` / gzip bytes | 3,308,070 / 848,296 bytes | Measure the complete isolated package, including every unique child. |
| Prepared full workflow p50 | 0.143–0.148 ms | Measure the same Agent + Finish graph with actual isolation. |
| DSL → first result p50 | 230.1–231.5 ms | Repeat compilation, preparation and execution for both backends in the same session. |
| Agent-only / parent step time | Not separately measured | Add spans to both backends; do not relabel the full workflow timing. |
| Local-server full execution | Not measured | Include queueing, real persistence and terminal publication. |

These historical macOS results are reference evidence, not a fresh comparison
against the implementation branch. A comparison is complete only when both
backends have run under the recorded matching conditions and correctness checks
pass; pending measurements must never appear as zero overhead.

### Qualification run checklist and deliverables

Use this sequence for the baseline/candidate comparison; retain exploratory
results separately from the qualification report:

1. Freeze the workload JSON, child definitions, inputs and build/runtime settings.
   Build both backend artifacts from the same revision. Record complete artifact
   hashes and sizes before timing, including each unique packaged child. Run
   correctness checks and verify actual isolated invocation counts first.
2. Run at least three independent paired sessions, alternating which backend runs
   first. For prepared execution, perform at least five untimed warmups and 1,000
   measured executions per backend and condition. Use fresh workflow state for
   first executions and a separate, explicitly primed state for checkpoint replay.
   Declare sample counts for expensive cold compilation and server/load cases
   separately; do not publish their p99 unless the tail-sampling requirement is met.
3. Capture uninstrumented full-run timings, then collect Agent and parent-step spans
   in a separate instrumented run. Use monotonic clocks for elapsed time. Report
   the instrumentation delta and retain timestamps sufficient to distinguish
   admission, child creation, invocation, transfer and cleanup where instrumented.
4. Repeat the server comparison with real isolated persistence and controlled local
   services. Capture both client-observed latency and accepted-request-to-persisted-
   result latency. Record startup, queueing and external service waits separately.
5. Run a repeated execution/cancellation soak under fixed concurrency and resource
   limits. Record its duration and iteration count, memory before/after quiescence,
   peak aggregate memory, retained handles and remaining tasks. Distinguish bounded
   cache retention from execution resources that should have been released.

Keep per-run distributions; do not average percentiles and label the result a
pooled percentile. Report independent-run spread alongside each run's p50/p95/p99,
and disclose any exclusions. Failed, rejected and timed-out requests need counts
and time-to-outcome distributions; their absence from successful-run latency must
be explicit. Percentiles of phase timings need not add up to total-run percentiles.

Publish `docs/research/workflow-performance-comparison.md` with a machine-readable
companion and retained raw sample artifacts. Include the configuration manifest,
baseline/candidate absolute values and deltas, correctness/isolation evidence,
sample counts, uncertainty, and a pass/fail/pending row for each agreed budget.
The existing report script currently compares only a subset of these metrics;
extend its schema and comparison checks before treating its output as the full
qualification report. Missing step spans, server measurements or candidate runs
keep the corresponding rows pending and cannot satisfy the P6 enablement gate.

### Reproduction

Build the real components with `scripts/build-agent-components.sh` if they are
not staged. From the worktree root, run the benchmark twice **serially**, with no
other build or benchmark running:

```sh
RUSTC_WRAPPER= cargo test --release -p runtara-workflows \
  --features direct-wasm-integration-tests --test direct_wasm_execute \
  workflow_performance_baseline -- --ignored --nocapture > /tmp/workflow-baseline-1.log 2>&1
RUSTC_WRAPPER= cargo test --release -p runtara-workflows \
  --features direct-wasm-integration-tests --test direct_wasm_execute \
  workflow_performance_baseline -- --ignored --nocapture > /tmp/workflow-baseline-2.log 2>&1
```

The [benchmark implementation](../crates/runtara-workflows/tests/wasm_performance_baseline/mod.rs)
refuses debug-profile execution and asserts default runtime retention. It runs
five warmups plus 100 measured fresh executions per small case, or 30 for
100-call/large-payload cases; compilation has three samples per case per run.
It uses the production parser/emitter/composition path and engine configuration,
with disk cache disabled, and requires `gzip` for transport-size measurements.
The owned epoch ticker is stopped at completion. All artifacts are temporary;
the graph and artifact hashes are retained in the report.

Use [the report script](../scripts/research/workflow_baseline_report.py) to
extract the `WORKFLOW_BASELINE_JSON` lines into JSON and Markdown:

```sh
python3 scripts/research/workflow_baseline_report.py \
  --logs /tmp/workflow-baseline-1.log /tmp/workflow-baseline-2.log \
  --output /tmp/workflow-baseline.json --markdown /tmp/workflow-baseline.md \
  --date YYYY-MM-DD --source-revision COMMIT_SHA --machine 'EXACT_CPU_AND_RAM'
python3 scripts/research/workflow_baseline_report.py \
  --compare /tmp/workflow-baseline.json /tmp/workflow-isolated.json
```

Replace the descriptive date/revision/hardware placeholders with actual values.
The report script rejects mismatched workloads and runtime configurations;
dependency changes are reported separately for attribution. It is a comparison
tool, not a performance acceptance gate.

### Required candidate comparison

Version 2 adds call-site metadata to both the raw and native package, so its size
cost grows with distinct call identities even when Agent component bytes are
deduplicated. Rerun the paired raw/gzip/native size and timing measurements with
this metadata included. The v1 report below does not measure v2 serialization,
validation, prepared-catalog memory or the persistence-backed scope factory.

The first Agent-only comparison now runs the real composed adapter backend.
The opt-in logical-context adapter v2 now has compiler wiring; the recorded
comparison still measures adapter v1. Repeat it for v2 and extend it as P2/P3
qualify all auxiliary calls and add isolated child graphs, without changing the reference workload definitions or correctness
checks. Continue asserting actual invocation counts and boundary identity; an
inline Embed or an absent Agent boundary cannot claim child-graph isolation.
Re-run baseline and candidate together on the deployment Linux host; retain the
macOS results as local evidence, not a cross-machine acceptance target.

For every case compare absolute values and percentage deltas for raw/gzip/logic
WASM size, serialized native size, JSON-to-WASM, native compilation, prepared
linking, cached full-run p50/p95, replay p50/p95 and JSON-to-first-result time.
The script provides the main latency/size deltas; retain the full metric table
for the remaining dimensions. Investigate changes in dependency hashes separately
from lowering changes. Never add the earlier per-Store microbenchmark cost to this
baseline and present the sum as an isolated measurement.

P5 must also add deployment-level measurements, with fixed conditions recorded:

| Additional measurement | Required boundary and controls |
|---|---|
| API submission → terminal result | Record server validation/registration, queue/admission, artifact load/preparation, execution and persistence separately; use isolated test tenant/data and report cold/warm artifact state. |
| Durable retry / Delay / WaitForSignal | Measure active compute separately from requested sleep, service time or human/signal wait; preserve wake/checkpoint outcomes. |
| HTTP and AI-like I/O | Use controlled response latency and payload sizes, then report real service time separately; no billing calls for benchmark comparisons. |
| Memory and throughput | Measure aggregate guest memory, process RSS, cache bytes, active children and queue depth at fixed concurrency 1/4/16/64. Largest-memory telemetry alone is insufficient. |
| Cancellation | Measure request → stopped/reaped → parent recovery at p50/p95/p99 under CPU/I/O/admission pressure, including completion races. |

These additional measurements have **not** been run here. They are the explicit
remaining production-comparison work, rather than being hidden inside “full
execution” or inferred from local synthetic persistence.

### Baseline extension verification

Both final release benchmark runs passed for all 11 cases; outputs, checkpoint
replay and real parallel-pool selection were checked. The ordinary execution
suite now includes one additional explicitly ignored manual benchmark. The 840
workflow/220 DSL baseline results below predate this test-only extension; they
must not be described as runs of the future isolated backend.

Workflow Clippy passed with all targets, the integration feature and `-D warnings`;
formatting and whitespace checks passed. The report regenerated byte-for-byte
from its JSON data. Identity comparison returned zero deltas, and changed input,
case inventory, build profile and second-run workload were all rejected by the
comparison checks. Documentation links resolve. DSL and production emitter/runtime
sources remain unchanged; this extension adds only the manual benchmark, reporting
script, measured artifacts and documentation.

## Original planning verification and requirement audit

Verified locally on 2026-09-06, against the current worktree:

- All **14** `Step` enum variants appear exactly once in the construct matrix.
  Nested placement and cross-cutting language features have separate requirements.
- All **17** named regression anchors in the gate table resolve to current test
  functions; linked local source/document paths exist.
- Fresh full workflow baseline: **840 passed** (565 library, 221 composed
  execution, 54 native integration); one existing doctest ignored. The execution
  harness requires staged components and asserts when they are unavailable; it
  does not silently skip that suite when a bundle is missing.
- Fresh DSL baseline: **220 passed** (219 unit tests and one doctest); two
  existing doctests ignored.
- No tracked changes in `runtara-dsl`, `runtara-workflows`, workflow WIT, runtime
  or stdlib relative to this worktree's HEAD. This goal added documentation only;
  earlier test-only host research remains in the worktree. Whitespace checks pass.

| Requested outcome | Evidence / completion boundary |
|---|---|
| Detailed selective-isolation plan | Structural selection, compiler integration points, complete child runtime method mapping, task ownership, command routing, data transfer, persistence, packaging and P0–P6 delivery sequence above. |
| No narrower support of existing constructs | Exhaustive construct matrix, custom-package lifetime compatibility, existing fallback/reference modes retained, and G1–G10 mandatory acceptance gates. Current baseline re-run is green. |
| No DSL changes | Explicit prohibition covering syntax/schema/defaults/validation; internal selection and runtime-control channel; DSL source diff empty and its suite green. |

This completes the **plan**, not implementation or proof that a future isolated
backend is regression-free. The isolated DSL backend, child runtime adapter,
durable attempt transitions, new differential tests and Linux qualification are
future implementation deliverables with explicit exit gates. No new production
behavior, remote CI run, deployment, commit or push was performed for this goal.
