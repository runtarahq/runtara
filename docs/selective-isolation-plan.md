# Cooperative workflow cancellation implementation plan

Status: revised 2026-09-06 following the decision to use standard WASM mechanisms
and the existing lifecycle signals. This replaces the
[selective isolation plan](selective-isolation-plan-superseded.md). The filename
is retained so existing links continue to reach the active plan.

This is a plan update, not a claim that cooperative cancellation is implemented.
The [current implementation record](cooperative-cancellation-implementation.md)
tracks this design. The [historical record](selective-isolation-implementation.md)
contains tests and code from the superseded design; they do not establish this
design's gates.

## Decision and guarantees

Update the existing implementation directly. Do not introduce opt-in product
features, alternative cancellation backends, runtime selectors or rollout flags.
Compare the current implementation against a separately built baseline revision;
existing test-only integration gates are sufficient. This supersedes the earlier
proposal to retain a flag-selected synchronous path during development.

Keep a single, normally composed `workflow.wasm`, with orchestration, cancellation
selection, timeout handling, cleanup and recovery in WASM. Use Component Model
async calls, waitable sets and subtask cancellation instead of a Runtara task
resource API. Reuse lifecycle signals for user requests. Preserve the existing
component lifetimes and composed instance pools; do not create a fresh Store for
each Agent call or extract EmbedWorkflow graphs merely to make them cancellable.

| Mechanism | Owner and behavior | Guarantee |
|---|---|---|
| User cancellation | Existing lifecycle signal reaches guest execution; WASM observes it, propagates cancellation, waits for cleanup and acknowledges completion | Controlled cancellation at supported cooperation points, not immediate termination on request receipt |
| Cooperative timeout | WASM races the operation against a clock deadline, then uses the same cleanup path with a timeout reason | Controlled outcome, but the deadline does not bound cleanup or termination time |
| Emergency full abort | Host interrupts and tears down the entire workflow execution after an explicit abort or configured cancellation grace period | Guest cleanup may be skipped; in-flight external effects can have an unknown outcome |

A local cancellation acknowledgement never means an HTTP server rolled back its
work. Completion can race with cancellation. Define and test the accepted outcome
before reporting success to the API. Preserve already committed checkpoints;
record an aborted run and uncertain in-flight work through existing lifecycle
state rather than pretending all partial effects were reverted.

No promise of force-stopping one uncooperative step while preserving its parent.
An infinite loop or code that never acknowledges cancellation can require a full
abort. If independent hard termination becomes a requirement later, treat it as
a separate design decision, not an implicit dependency of cooperative cancellation.

## Standard mechanisms and feasibility gate

The repository already exports async-typed Agent and workflow `invoke` functions.
The workflow lifecycle WIT explicitly says its lift uses the synchronous ABI.
Async typing alone does not establish guest cancellation delivery or cleanup.
Inventory the actual ABI used by each generated call/export and by each guest
language binding before changing it.

Use the pinned Rust toolchain and Wasmtime dependency. Verify the supported,
standardized subset against those exact versions; do not infer support from newer
online docs or enable experimental ABI extensions silently.

- Prefer Component Model async call handles, waitable sets, `subtask.cancel`,
  cancellation delivery to a supported guest callback/binding, and
  `subtask.drop` after resolution. Do not wrap them in a second host task registry.
- A callee must acknowledge cancellation or return; a request does not force it
  to stop. Follow the ABI's borrowed-resource and subtask-resolution rules.
- Centralize built-in Agent callback bindings, invocation dispatch and error
  conversion in `runtara-agent-macro`, reusing existing `#[capability]` metadata.
  Apply this to earlier migrations too. Keep actual asynchronous helper/I/O calls
  explicit; an annotation does not make blocking code cooperative. Verify the
  shared expansion across every built-in Agent and preserve metadata, coercion,
  input conventions and provider errors. No per-Agent cancellation backend or
  opt-in annotation is needed.
- Verify standard cancellation delivery end to end: callback bindings for
  built-in agents, or cancellable waitable-set waits/yields for emitted workflow-agents.
  A synchronous lift or async export type alone is not sufficient proof.
- Verify whether cancellation itself can block the waiting guest, which async
  cancellation forms the pinned runtime supports, and how the emergency host
  grace timer remains effective during that wait. Do not depend on optional
  async-cancel extensions unless qualified on the pinned stack. Do not add an
  opt-in backend or product flag to select them.
- WASI P2 pollables can wait on I/O and timers inside cooperating agents. P2
  resource polling does not by itself cancel an arbitrary component call.
- Keep existing P2 I/O imports where they can participate safely. Adopt P3 I/O
  only where needed and supported; a wholesale P3 migration is not assumed.
  Check pending headers, response-body reads, resource disposal and host cleanup.
- Carry lifecycle notifications over the existing runtime signal interface. If
  it needs an awaitable operation, expose a standard async/future/stream shape,
  or initially use bounded polling with a clock. This is signal transport,
  not a start/cancel/join execution service.

First prove a normally composed parent and HTTP agent can await completion and a
signal, cancel cooperatively, release resources and continue parent WASM while an
independent sibling completes. Repeat using emitted DSL, not only handwritten
WAT. A blocked endpoint must withhold headers until cancellation; another fixture
must send headers and then block the body. No custom isolated executor may be
used to make either proof pass.

If the pinned stack cannot implement the standard path, document the exact
missing binding/runtime capability and a scoped upgrade proposal. Do not silently fall back to the superseded isolation design.

Primary references, checked 2026-09-06:

- [Component Model cancellation](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#cancellation): cooperative acknowledgement, supported delivery ABI, and no forced thread termination.
- [Canonical subtask operations](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md): resolution, cancellation and handle-drop contracts, including extension gates.
- [WASI P2 polling](https://raw.githubusercontent.com/WebAssembly/wasi-io/v0.2.0/wit/poll.wit): multiple readiness sources and clock-based timeouts.

## Responsibilities and signal semantics

| Component | Responsibility |
|---|---|
| `runtara-server` | Authenticate and authorize the request, submit the existing lifecycle command, expose requested versus completed status |
| `runtara-core` | Persist and deliver lifecycle signals using existing command IDs/acknowledgements; preserve checkpoints, wake and terminal-state rules |
| `runtara-environment` | Own the whole run, deliver lifecycle notifications, apply configured emergency-abort grace, publish terminal state after execution cleanup |
| `runtara-component-host` | Implement WASI/runtime imports and whole-run resource limits/abort; rely on the engine's standard async machinery |
| `runtara-workflows` | Emit cooperative waits, deterministic cancellation/completion selection rules, propagation, timeout reasons and existing recovery behavior |
| Workflow runtime/stdlib and agents | Receive cancellation at cooperation points, cancel or dispose pending I/O according to its contract, release borrowed/owned resources and acknowledge completion |
| Parent workflow WASM | Own graph execution, retry policy, sibling handling, cleanup order, recovery and the decision to continue or terminate |

Reuse lifecycle `Cancel`; do not consume application `WaitForSignal` payloads or
reserve an arbitrary business signal name. Existing root Stop still targets the
whole workflow. Pause and shutdown retain their distinct lifecycle semantics.

Implementation evidence now joins the environment Stop handler to composed HTTP
cleanup and real persistence, plus independent grace abort of infinite WASM
invocations/initializers and a standard cancellation callback stalled in cleanup
I/O. The server's Stop/cancel methods share that handler. Full authenticated-server
E2E, remote-owner routing, blocking native-call qualification and the remaining
timeout gates are still required; see the implementation record.

The first end-to-end user control targets the root. Cancelling a particular step
requires a later, explicit extension of the same control mechanism: an opaque
logical invocation address distinguishing loop iterations, nested calls and retry
attempts. This is not provided by the current root Cancel signal. Keep resolution
of that address to active standard subtasks in guest state; host authorization and
command transport must not become a second execution graph or task manager.

Audit acknowledgement timing. Current `RuntimeHost::is_cancelled` can acknowledge
the command as soon as it observes it; that is not evidence that guest cleanup
finished. Separate request observation from terminal cancellation publication for
the new path, preserving old-artifact semantics through versioned compatibility.
Do not have multiple parallel waits independently consume a root command. Guest
execution must propagate one observed root intent to every active operation.

Define these rules in executable tests before compiler integration:

1. A request is pending until observed. If completion was already accepted, a
   later cancellation cannot rewrite it. Specify the tie rule for simultaneously
   ready events and apply it consistently at the guest and persisted boundary.
2. Once cancellation is selected, launch no new ordinary work or automatic retry
   in that cancelled scope. Cleanup and an explicitly defined recovery path may
   run. Root cancellation cannot be swallowed by a step's ordinary `onError`.
3. A locally cancelled step may follow its existing error/recovery mechanism;
   cancelled work must not automatically restart through default retry policy.
   Define timeout retryability separately and preserve existing timeout contracts.
4. Report cancellation complete only after the cooperative cleanup contract is
   satisfied. A cleanup failure or expired grace triggers full-run abort, not a
   fabricated successful cancellation.
5. Grace measures elapsed time from the defined accepted request/deadline event
   using a monotonic clock. Keep the host emergency timer independent of guest
   cooperation. CPU work must yield/check at documented points; no promise of
   guest observation inside arbitrary uninstrumented loops or blocking imports.

No new authored cancellation-handler syntax is introduced by this plan. Specify
behavior using existing recovery facilities and compiler-generated cleanup first.
If a dedicated user-authored handler is needed, propose its DSL semantics
separately rather than implying one already exists.

## Compatibility and compiler changes

Preserve all accepted DSL constructs, schemas, defaults, connection resolution,
error envelopes, retry counts, checkpoint keys, replay behavior, signal/wake sets,
event/debug behavior and graph-required ordering when cancellation is absent.
Do not reset agent globals or initializers by changing instance lifetime.

Keep E128 Agent/Embed timeout rejection until those exact contracts are proven
and a deliberate validation change is made. Existing capability timeouts,
AiAgent turnTimeout and nested deadline frames retain their meaning. Test zero,
overflow, inherited budgets and restoration after nested recovery.

| Area | Change |
|---|---|
| Agent sequential and parallel calls | Use standard asynchronous calls and cancellable guest waits; retain mapping, connection binding, result shaping, checkpoints and retries around the call |
| EmbedWorkflow | Preserve inline/composed execution and scopes; propagate cancellation through nested guest control flow without extracting a separate child Store |
| Split, branches and While | Preserve concurrency windows, ordering and scope; propagate root cancellation to active peers, with targeted cancellation affecting only the selected scope when supported |
| AiAgent | Preserve the WASM conversation/tool loop; cover provider, tool and memory calls and prevent new calls after cancellation selection |
| Delay, WaitForSignal, pause and replay | Preserve suspension as a distinct outcome; reuse wake/signal machinery and retain unrelated waits |
| Local operations | Add bounded cooperation points where needed without changing computation results; infinite/uncooperative execution remains a full-abort case |
| WIT, bindings and component composition | Use standard imports and async ABI; preserve one normally composed artifact and existing instance pools; version changed runtime signal semantics/capabilities explicitly |

Review all exhaustive DSL matches and the full 14-variant inventory. Preserve
large JSON values and run-local arena handles through existing component calls;
no new cross-Store transfer protocol is needed. Preserve tenant isolation and
opaque credential handles; cancellation introduces no raw credential access.

Keep old artifacts executable and parked runs resumable under their original
contract. Never silently recompile a parked workflow. Pure workflows keep their
no-runtime mode where currently supported. Existing CLI/reference/export modes
remain covered; detect missing runtime capabilities before invocation.

## Superseded implementation and migration

Stop extending the experimental `runtara:workflow-execution/tasks` API, custom
start/cancel/join/release handles, child Store supervisor, per-call isolation
selector and custom child catalog as the cancellation solution. Standard engine
subtasks provide execution bookkeeping; guest compiler state provides workflow
control. Ordinary whole-run host supervision remains necessary.

The separate invocation lease/attempt ledger and cancellation write fencing are
not prerequisites for cooperative cancellation. Reuse existing lifecycle
persistence and checkpoints. Test restart and duplicate-signal behavior against
that contract; do not promise exactly-once effects or introduce durable targeted
cancellation guarantees without a separately justified requirement.

Preserve historical experiments and measurements as evidence. Inventory which
experimental changes are actually selected or referenced before removing them;
retain independently useful fixes only with their original justification/tests.
Determine whether any isolated artifacts have been registered or parked. If so,
keep their decoder/runtime support until drained; do not delete support based on
an assumption that an opt-in path was unused. Keep new compilation on standard
composition. This document update does not delete code, migrations or artifacts.

## Tests and release gates

Run the existing full emitted-workflow harness against the separately built
baseline revision and current implementation with identical fixture inputs, scripted services, signal schedules and
preloaded checkpoints. Verify actual standard cancellation events and resulting
cleanup; a test that routes through isolated tasks or waits for the remote server
to finish cannot establish this design. Compare outputs, errors, request counts,
retries, checkpoints, wake sets and required event ordering. Do not shadow-run
real external effects for a differential comparison.

| Gate | Required evidence |
|---|---|
| G1 Language and compatibility | Complete DSL/audit corpus and all 14 constructs; unchanged accepted/rejected inputs, schemas and no-cancel behavior |
| G2 Standard ABI | Real composed parent/agent cancellation; correct callback delivery, resolution, borrowed-handle release and repeated-call state; no custom task imports/catalog |
| G3 I/O interruption | Cancel during pending headers, blocked body, connect where controllable, and nested provider/tool calls; demonstrate local cleanup with remote work still pending |
| G4 Races and signals | Pre-start, queued/backpressured, in-flight, already-completed, simultaneous-ready and duplicate commands; root versus targeted scope; request/ack/publication ordering |
| G5 Timeouts | Same cleanup as user cancellation with distinct reason; inherited/zero/overflow budgets; completion races; grace and non-cooperation escalation |
| G6 Graph and parallel behavior | Nested Embed, Split/branches/While, AiAgent calls; unaffected sibling survives targeted cancellation; root cancellation stops all; preserve overlap and result order |
| G7 Persistence and suspension | Existing checkpoint-hit/retry/replay behavior; no extra completed side effects; cancellation around terminal/checkpoint writes; pause/shutdown/Wait/Delay and unrelated wake preservation |
| G8 Emergency abort | CPU infinite loop, infinite initializer, ignored cancellation and stalled cleanup; full execution ends without guest recovery being claimed; resource accounting and uncertain outcome are correct |
| G9 Artifact compatibility | Standard composed artifact without isolation catalog; old artifact/replay/export modes; supported runtime versions; native cache identity/validation and pure no-host case |
| G10 Capacity and performance | Repeated cancellation/resource soak, component-state preservation, bounded pending operations, Linux latency/throughput/RSS and paired size/timing report |

Use the pinned toolchain. Run `cargo fmt --all -- --check`, focused tests and
all-target Clippy for affected crates. For guest/WIT changes run
`scripts/build-agent-components.sh` and the relevant component-host/workflows
integration features from `.github/workflows/ci.yml`. Widen to core/environment/
server lifecycle tests and isolated test databases when those boundaries change.
Finish with a local server and controlled HTTP service exercising real signal
submission, cooperative cleanup, status publication and emergency escalation.
Record exact commands, enabled features, executed test cases and skipped checks.

## Baseline measurements and comparison

Keep the existing eleven-workload benchmark corpus, including one
`utils/random-double` Agent followed by Finish. Separate DSL defaults,
`durable: false`, durable first execution, checkpoint replay and event tracking.
Verify numeric range/count and cached replay equality without requiring fresh
random draws to be byte-identical.

Historical [baseline](research/workflow-performance-baseline.md) and
[isolation comparison](research/workflow-performance-comparison.md) remain useful
reference data. Their isolated-Store candidate is superseded; it is not a
measurement of the cooperative design. Preserve the existing reports rather than
relabelling their results. Publish fresh results as
`docs/research/workflow-cooperative-cancellation-comparison.md` and `.json`, with
raw samples and a machine-readable configuration manifest.

The [first interim comparison](research/workflow-cooperative-cancellation-comparison.md)
now covers 14 workloads in three paired release sessions, with 1,000 prepared
samples per condition. It does not close the full qualification gate. The
100-step chain has 8.0% more raw `.wasm` bytes and 2.26–3.64 times the
compile-through-first-result median; prepared execution changes by +1.5–4.1%.
Shared emitted wait/poll/cleanup helpers now preserve caller-local state,
entry-ABI returns and cleanup-before-ack tests. Their code-size and runtime
effects must be measured before closing P2 performance work.
Re-run paired measurements after optimization and after the remaining migration;
missing server, cancellation, instrumentation and capacity metrics remain pending.

The [shared-helper follow-up](research/workflow-cooperative-shared-waits-comparison.md)
reduces the 100-step raw artifact overhead from 8.0% to 1.75% above upstream,
while adding 4,019 bytes to the prior single-step cooperative artifact. Its three
paired timing sessions are exploratory: host load varied substantially and the
cold compilation regression remains unresolved. Repeat timing on a controlled
machine before performance acceptance. Evaluate unused-helper elimination and
narrower helper state transfers without adding an alternate execution path.


| Metric | Required boundary |
|---|---|
| `.wasm` bytes | Entire normally composed distributable artifact; raw and gzip with identical compression settings; report workflow logic/dependencies without double counting |
| Native artifact bytes | Entire serialized prepared component/cache representation |
| Compilation and preparation | DSL validation/emission, composition, native compilation and prepared linking separately and combined |
| Single Agent service time | Invocation entry through response/cleanup, excluding surrounding workflow work |
| Single parent step time | Mapping/checkpoint lookup through invoke, cleanup, retry/error handling and result checkpoint; distinguish service time from step time |
| Prepared full execution | Fresh workflow Store, instantiation, entire graph, runtime callbacks, terminal outcome and teardown; exclude compilation |
| Cold first result | DSL through artifact preparation and full execution; report engine startup separately |
| Local-server full execution | Client submission through persisted terminal outcome, including auth/queueing/persistence; separate startup and controlled external service delay |
| Cancellation latency | Request acceptance to guest observation, observation to cleanup completion, and completion to published terminal state; report timer lateness and emergency-abort latency separately |
| Capacity | Throughput, p50/p95/p99 with adequate samples, peak/steady RSS, retained memory/handles after quiescence, CPU and signal-poll/DB query cost |

Measure no-cancellation overhead as well as cancellation. Cover Finish-only,
single random-double, chains of 10/100, sequential/parallel Split, nested Embed,
CPU-only While/Split at small and large iteration counts (root and published
workflow-agent forms), small/16-KiB-boundary/MiB payloads, real HTTP header/body waits, replay and root
abort. Assert concurrent overlap and standard ABI use; no sequential fallback.

Run the baseline and current revisions on the same host with matching dependency,
compiler, runtime and fixture settings wherever unaffected by the change. Use
separate Cargo target and component output directories for each worktree/revision;
do not share artifact paths even when package versions match. Verify the built
component ABI before timing so a cached synchronous artifact cannot stand in for
the cooperative implementation. Document all necessary differences explicitly;
no product backend selector is introduced. Record hashes, versions,
machine specifications and actual ABI modes. Keep legacy and candidate runs paired across
at least three independent sessions, alternating order. Prepared runs require at
least five warmups and 1,000 measured executions per condition. Declare separate
sample counts for expensive cold/server/load cases and do not publish unsupported
tail percentiles. Report each session's distribution and spread, not averaged
percentiles. Include failure/rejection/timeout counts and latency distributions.

Capture uninstrumented totals and instrumented phase spans separately; report the
instrumentation cost. Collect Linux capacity data before rollout. Establish
explicit deployment budgets for size, latency, memory, throughput and cancellation
responsiveness from paired measurements; missing metrics are pending, never zero.
Do not claim real-time termination guarantees from observed cooperative latency.

## Implementation sequence

| Phase | Deliverable and exit criterion |
|---|---|
| P0 Inventory and baseline | Audit current async ABI and signals, snapshot no-cancel semantics, identify experimental dependencies/artifacts, capture fresh baseline |
| P1 Standards feasibility proof | Composed HTTP cancellation and sibling preservation using pinned standard primitives; prove callback delivery and cleanup; record any upgrade blocker |
| P2 Guest cooperative execution | Wire generated waits and agent cleanup, retain component lifetimes/composition, prove single-step and nested/parallel parity; publish first size and timing comparison |
| P3 Signals, lifecycle and timeout integration | Reuse root signals, define acknowledgement/terminal ordering and timeout reasons, implement grace/full abort; targeted control only with explicit same-mechanism scope semantics |
| P4 Qualification and cleanup | Complete G1–G10 and local-server E2E, publish paired report; retire superseded paths safely while retaining any required old-artifact support |
| P5 Release | Release the updated existing implementation after qualification; retain supported active/parked artifact contracts and document rollback compatibility |

Complete the gates before merging/releasing the updated implementation. Small tested commits
should separate standard ABI/binding changes, emitter behavior, lifecycle changes
and cleanup of the superseded experiment. No independent hard-cancel mechanism,
custom host task manager or new invocation ledger is part of this plan.


### Nested callable progress (2026-09-07)

Non-durable workflow-agents now receive parent cancellation at standard
cancellable waits, clean their nested sequential/parallel calls and return to
the composing caller without owning root lifecycle signals. Local Agent backoff
uses the existing awaitable timer and the same guest cleanup. Production safety
analysis admits qualified non-durable Agent graphs; durable suspension and
unsupported runtime-dependent closures remain rejected. See the implementation
record for proofs and regression results. G2/G6 have additional coverage, not
blanket completion: durable/nested Embed/While, targeted cancellation/deadlines,
blocking cooperation, resource soak and new paired measurements remain open.


### Loop and inline-child progress (2026-09-07)

Emitted While and sequential Split now cooperate between iterations: published
workflow-agents use the standard cancellable yield and root workflows use shared
lifecycle polling/cleanup. Runtime-free While compilation and normal outputs are
covered, as are legacy error routing and cleanup before acknowledgement when a
While has a pending HTTP sibling. Tests also cover two inline Embed scopes,
partial bodies and While/parallel child shapes. This expands G2/G6 evidence;
bounded cooperation inside Agent/stdlib/native calls, all construct combinations,
targeted timeouts, durable callable suspension and fresh size/time/polling-cost
measurements remain required. A per-iteration cooperation bound is not a fixed
latency guarantee for a large or blocking iteration.


### Root retry progress (2026-09-07)

Root non-durable Agent backoff now shares the published-agent async timer wait.
Cancellation during ordinary and recognized rate-limit waits is covered with
real HTTP/Slack agents, alongside no-cancel delay and retry-count compatibility.
Durable retries keep their checkpoint-and-park behavior. Remaining blocking wait
sites include Embed/Split retry helpers and legacy/capability WaitForSignal
polling. Production root WaitForSignal already parks on a signal after a miss
and must retain that behavior. The lower-level non-durable Delay emitter blocks,
but production rejects that graph to avoid holding a runner; retain this
acceptance boundary during cancellation work. Agent-free wait graphs need
shared-wait import/helper provisioning where an accepted construct requires it. Root
backoff retains a Store and uses the existing one-second lifecycle poll, which
must be included in the capacity and signal-service cost qualification. This
expands G4/G7 evidence; it does not retire E128 or complete timeout gate G5.


### Composite retry progress (2026-09-07)

Non-durable root Embed/Split backoff now shares the Agent timer/wait/cleanup
helper. Agent-free graphs derive required timer imports from their existing
manifest, including nested and preloaded child graphs; zero-retry graphs do not
add timers. Returned and on-disk scaffolding preserve the timer requirement after
composition. Tests cover cancellation, zero delay/retries, retry exhaustion,
nested While/Embed placement and real HTTP errors inside Split. Durable root
retry parking remains intact.

AUDIT-12 identifies existing formatted-Agent-error incompatibilities: Embed
fails JSON parsing before backoff, while Split loses rate-limit classification.
Reference tests reproduce these with the old blocking wait too; preserve their
current results while separately qualifying a structured error contract. Do not
count those negative cases as successful cancellation coverage. Publication gates
for callable Split/Embed retries remain until their full closure is qualified.
P3 timeout integration, P4 lifecycle/resource/capacity checks, updated interactive
audit coverage and fresh performance comparisons remain required.

### Published Split retry progress (2026-09-07)

The callable publication gate now permits non-durable Split retries in a complete
closure without root runtime requirements. Two nested workflow agents propagate
parent cancellation to the shared canonical timer wait without consuming root
signals. Nine execution tests cover cancellation, success, recovery, zero retries
and existing provider classification. Requested parallelism retains the documented
sequential fallback for Split-level retries; no new concurrency claim is made.
Split timeout features now participate in runtime ownership analysis, including
nested scopes, and negative publication cases cover the listed runtime requirements.
Embed retries, durable callable suspension, targeted/time-based cancellation,
full lifecycle/resource qualification and current paired measurements remain open.

### Deadline contract progress (2026-09-08)

A standard Component Model fixture now defines deadline selection before emitter
integration: observe both readiness sources, accept completion when both are
observed ready, otherwise select expiry and resolve/drop the target before
continuation. A normal value returned during cancellation cleanup cannot undo
selected expiry. Nine tests cover real HTTP waits, unused timer cleanup,
component reuse, zero remaining time, maximum timer duration and deliberately
broken race handling. The sibling is fixture I/O. This is G5 contract evidence,
not a released DSL feature: scoped deadline/reason propagation, inherited/durable
budgets, recovery routing, root-cancel interaction and emergency grace still need
compiler and runtime qualification. E128 remains unchanged.


### Recovery prerequisite · structured Agent errors (2026-09-08)

Correct AUDIT-12 before routing new scoped timeout outcomes through recovery.
Newly compiled Agent failures must retain structured policy fields through both
fresh results and durable attempt replay. Embed retains scope/child diagnostics
and propagates the originating code and retry fields; composite retry respects
explicit nonretryability. This deliberate failure-contract correction is an
exception to the earlier no-cancel parity baseline and requires explicit tests
for recovery payloads, component error exports and unchanged replay side-effect
counts. It does not relax E128 or establish scoped deadline/grace support.


### Published Embed follow-up · 2026-09-08

The existing callable path now admits non-durable Embed retries when the complete
supplied child closure requires no root lifecycle runtime. Share this closure
analysis between publication and import omission, and use the existing canonical
boundary yield. Nested failure/retry frames restore parent graph identity and
child input before recovery or another attempt. Execution coverage includes
HTTP/Slack retries through two published levels, nested inline Embed, root nested
recovery, and pure-child normal output. This expands G2/G6 evidence; it does not
qualify failing Agent-free callable backoff, durable callable suspension, scoped
timeouts/grace or the remaining compatibility/performance gates.

### Pure callable retry qualification · 2026-09-08

Qualify failing Agent-free published Embed/Split waits with a plain stdlib
integer-coercion error. Correct Embed's assumption that every child error is JSON
in the existing shared wrapper; keep raw diagnostics and existing generic retry
policy, without extracting policy from JSON fragments in text. Tests cover
parent cancellation during backoff and root/published zero-retry, zero-delay and
delayed recovery. This closes the failing pure-callable backoff gap identified
above; it does not establish scoped timeout/grace, durable callable suspension,
exact cancellation latency or the remaining compatibility/performance gates.

### Shared emitted deadline outcome · 2026-09-08

The shared Await emitter now has an owned deadline-subtask input and a distinct
scoped-timeout outcome. Deterministic execution of the generated helper checks
readiness ties, final selection during cleanup, root/parent cancellation and
preservation of an enclosing sibling window. This replaces the missing emitter
mechanism; it does not yet wire DSL scopes to that input/outcome. Current callers
supply no deadline, and E128 remains unchanged. Integrate the owning scope's
remaining budget and recovery target before enabling it: discard a late result,
skip cancelled work's retries, preserve durable deadlines, restore outer context
and keep unrelated parallel work live. Cleanup grace and the remaining G1–G10
qualification still apply.


### Agent invocation/retry deadline wiring · 2026-09-08

The common sequential Agent lowering now supplies an owning timer and consumes
Await's timeout outcome before retries or recovery. It preserves one durable
budget across failed attempts and replay, caps backoff wakes, bypasses old budgets
on a result-cache hit, and reports a nonretryable typed timeout after cleanup.
Composed HTTP tests cover request cleanup, root cancellation bypassing recovery,
zero/overflow budgets, later attempts, early/expired replay, and malformed state.
See the implementation record's “Agent deadline integration” section for exact
coverage and limits.

E128 remains: runtime-free/monotonic clocks, inherited scope and recovery routing,
parallel scoped ownership, preparation interruption, and cleanup grace are still
required. No feature flag is added. The temporary internal parallel eligibility
restriction must be removed once scoped window deadlines are qualified. This is
progress on G5/G7, not completion of either gate or the full G1–G10 plan.


### Standard monotonic clock and published budgets · 2026-09-08

Live Agent budgets now use the standard WASI monotonic clock. Durable replay
retains the epoch deadline and converts its remaining duration at scope entry;
non-durable published workflows need no lifecycle runtime. The resolved standard
WIT inputs are reused directly, and clock import requirements come from a shared
inventory that includes inline nested definitions. There is no product flag.

Composed tests cover one/two published layers, root cancellation, timeout recovery,
backoff expiry, zero/large budgets, and inline While inventory. Root tests verify
that forward/backward fixture wall-clock changes do not change a live budget.
The arithmetic test executes the actual emitter with controlled clock readings.
Persisted time while parked still follows the existing epoch/scheduler contract.

Inherited ownership and sequential unwind/recovery are implemented in the next
stage below. Parallel sibling survival, complete timeout ownership for Embed/AI,
interruptible preparation and cleanup grace remain required before removing E128.
The full G1–G10 gates and paired performance/E2E/soak work remain open.


### Inherited deadlines and recovery ownership · 2026-09-08

The emitter now carries the earliest enclosing While/Split budget as guest locals:
a manifest-wide owner, monotonic start/duration and error payload. Loop and Embed
frames restore it; a separate selected reason survives unwind. Parent expiry
bypasses child Agent handlers, Split aggregation/retries and Embed attempt
checkpointing. Only the owning loop consumes the reason before recovery. No
host task registry, new WIT cancellation interface, product flag or extra Store
is involved. Existing standard Await owns and resolves the pending timer/I/O.

Live loop checks use the standard monotonic clock, including untimed inner loops.
Persisted epoch deadlines still govern parked/restarted time. Agent retry delays
intersect the enclosing live budget, and persisted retry deadlines are clamped
before saving as well as before returning wakes. Completed-loop replay remains
outside that loop's old budget. Existing zero-disabled loop syntax is unchanged.

Qualification includes nested shorter/longer budgets, durable and non-durable
Agents, ordinary HTTP errors/success, root Cancel, sequential Split aggregation
and retry, untimed CPU loops with a frozen epoch clock, recovery after expiry,
AI single-shot/turn/load/summarize/save I/O, and two nested Embed levels with
pending headers/body. Existing AUDIT-05 replay, aggregation and mixed-scope tests
remain required, including a separate real-overrun and wall-clock-jump control.
The implementation document records completed commands and remaining limits.

Next: qualify inherited timers with live parallel windows and sibling survival,
complete own Embed/AI/tool deadlines and their retry/preparation waits, then
bounded cancellation cleanup and the remaining public Stop/E2E/soak/measurement
work. E128 remains until the complete contract passes; this stage does not close
G5, G6 or the full G1–G10 matrix.


### Parallel scope deadlines · 2026-09-08

Enclosing deadlines now reach the existing parallel wait set in Split, branch
scheduler and depth-wavefront paths. Expiry resolves the window before owner
recovery, balances its pause deferral and skips child recovery/retry. Ready
completions win deadline ties; internal timers cannot become Agent results.
Launch/assembly boundary checks prevent new work after an observed expiry.

Split-owned timeouts now retain the requested concurrent window for otherwise
eligible bodies. Production HTTP tests prove overlap, header/body cleanup,
reverse-completion/input-result ordering, ordinary failure recovery and durable
replay. A delayed fast-branch assembly test requires cleanup of its pending peer
without launching the next request. Deterministic helper tests cover cancellation
resolution, event ordering, root/parent priority and handle ownership.

The wait owns a fresh timer until it returns an event, so shared-wait size and
per-event timer overhead must be included in the controlled comparison. This
stage does not close individual timed-Agent sibling survival, nested concurrent
scope ownership, interruptible preparation, own Embed/AI/tool budgets, cleanup
grace or G1–G10. E128 and the separate remaining parallel eligibility constraints
remain until their contracts are implemented and qualified.

### Connection preparation progress · 2026-09-08

Connection metadata lookup now uses the shared emitted cancellation/deadline
wait before Agent invocation. Sequential Agent/AI, inline Embed, published-child,
Split and branch paths share this implementation. Expiry skips invocation and
unwinds to its owning scope; a parallel window resolves pending peers first.
This covers pending metadata headers and partial bodies, not every possible
preparation operation or CPU-bound transformation.

The resolver interface advances to async-typed 0.2.0. The host retains 0.1.0
bindings for existing binaries; new compilation selects 0.2.0 without flags.
Both versions execute against the same HTTP resolver and per-run caches.
No host graph scheduler or per-agent cancellation wrapper is introduced.

The published-child timeout fixture puts the budget on its calling parent;
runtime-bearing published graphs remain subject to existing safety validation.
Own Agent preparation budgets have private-emitter coverage while E128 stays
in force. This adds G5/G6 evidence without closing their remaining gates.


### Embed-owned budget progress · 2026-09-08

The normal inline Embed run-plan now owns a total cooperative budget across
child I/O and retries. It uses the same monotonic arithmetic, durable deadline
keys, scope ownership and wake clamping as the existing implementation. Expiry
resolves child work before `EMBED_TIMEOUT` recovery; an earlier parent deadline
bypasses child handlers. Durable result hits remain complete on later replay.

Private-emitter coverage includes nested Embed and overlapping Split children,
zero/saturating budgets, root Cancel, ordinary errors, absent recovery, malformed
state, early Delay resume and durable/non-durable backoff. See the implementation
record for exact results. E128 remains: Embed-as-tool budgets, runtime-free
publication with own Embed deadlines, completion races and cleanup grace still
require qualification before releasing the authored timeout field. This advances
G5/G6/G7 without declaring them complete.

### Inline Embed tool scopes and budgets · 2026-09-08

Reuse the normal Embed budget entry, exit and timeout capture helpers for an
Embed invoked by the AI loop. Use the existing workflow-agent tool identity
formula (AI step, advertised label and replayed call counter) for its caller
source, child checkpoints, result and total budget. Preserve the source's
manifest definition path, loop identity and user variables separately.

The candidate saves a completed tool result before the next tool runs. Its own
expiry becomes `EMBED_TIMEOUT` feedback after scope restoration; inherited
expiry and root Cancel leave the AI loop without another model call. A completed
result bypasses deadline loading on replay. Tests must include two calls to the
same tool, both timeout/success orderings, a partially completed turn that parks,
early resume, and both parent/child budget orderings.

Compatibility: new inline Embed tool artifacts now use a per-call checkpoint
namespace. This corrects collisions between repeated calls; it is not a migration
of a parked artifact's checkpoints. Existing composed binaries retain their
embedded implementation. Published workflow-agent scope keys remain unchanged.

Keep G5/G6/G7 and E128 open. The resume fixture currently scripts the same model
reply on each attempt. Persist the model reply before dispatching its tools so a
changed response on replay cannot assign a cached result to different arguments.
Also qualify AI-inside-Embed-tool state/arena frames, nested/parallel children,
the remaining preparation operations, runtime-free
publication, completion races and cleanup grace before claiming complete tool
support. The full performance, server E2E and soak requirements are unchanged.

Next replay qualification: save the returned AI turn before starting its first
tool. Reuse the existing guest checkpoint machinery with a distinct key from the
completed-turn snapshot. A checkpoint write failure must prevent tool dispatch.
Resume must restore the exact tool IDs, order and arguments without another
model request; test a different next provider reply and a failure between tool
calls. This applies once in the shared AI loop to every tool kind, rather than
adding separate persistence code to individual Agents or transports.

### Pending AI turn decisions and checkpoint failures · 2026-09-08

The shared durable AI loop now checkpoints the model's returned decision under
`ai_turn_response` before dispatching any tool. Completed-turn snapshots retain
their existing keys and semantics. Resume restores the saved decision, including
its tool IDs, ordering, arguments and conversation, without another model call
for that pending turn. This resolves the repeated-response limitation recorded
above for inline Embed tools; Agent and WaitForSignal tools use the same path.
A pause immediately after saving the response leaves all tools unstarted.

The existing shared checkpoint lookup/save helpers now propagate storage errors
instead of treating a failed read as a miss or continuing after a failed write.
A malformed saved decision returns non-retryable `AI_TURN_RESPONSE_STATE` before
dispatch. The corruption, storage failure, signal-resume and partially parked
turn tests are in `compile/embed_tool_deadline_tests.rs`; verification is recorded
in the implementation document. AI-inside-tool frame/arena isolation and the
remaining timeout, publication, race and grace gates remain open.

Extend paired measurements with durable AI loops of 1/10/100 turns, one and
multiple tools per turn, small and growing conversations, and pending-turn
resume. Report response-checkpoint lookup/write latency and bytes separately
from completed-turn snapshots. The new path adds one decision lookup per live
pending turn and one response write per fresh model result, retaining another
copy of that response until normal checkpoint cleanup. Cache hits do not rewrite
the response. Quantify database growth, guest validation work, cold artifact cost
and avoided provider requests on resume; these costs are not yet measured.

For the next nested-AI qualification, exercise an outer AI loop whose inline
Embed tool contains another AI loop, including two tools in one outer turn,
multiple inner turns, repeated outer calls, errors, parent/own cancellation and
park/resume. Verify the outer conversation, pending results, call counter and
heap watermark after the child returns; test large histories near the arena
interning boundary. Reuse the existing guest frame discipline at the child
boundary so this does not become another per-Agent wrapper or host task layer.

The shared checkpoint helper fix is not a complete raw-call inventory: ordinary
Agent attempt replay and parallel Split attempt replay still read checkpoints
outside that helper; debug checkpoint handling is separate too. Add fault tests
for those sites and for the parallel launch paths that defer errors until their
window is drained. Preserve peer cleanup before returning a storage failure;
do not turn a read error into a new attempt or silently report successful
checkpoint completion. Their qualification remains part of the durable/error
and resource-cleanup gates.

### Nested AI inside Embed tools · 2026-09-08

The immediate nested-AI reproduction is fixed: the Embed attempt boundary saves
and restores the caller's AI state using an explicit guest frame, preserving its
conversation, pending results, counters and heap watermark. The child result is
excluded from restoration. Tool planning now selects Embed by the current
graph's declared target type, preventing a same-ID inner Agent/Wait tool from
being mistaken for the outer Embed and recursing during native compilation.

Six composed tests exercise repeated local IDs, two tools in one outer turn,
more inner/outer turns, 16–64 KiB histories, child provider errors, root Cancel,
both timeout owners, nested signal resume and child collection with an outer
64 KiB interned value. Existing completed-turn and per-call keys are preserved.
No host task layer or product switch is introduced.

Remaining follow-up: qualify AI in other inline callbacks (`onWait` and recovery
handlers), deeper mixed parallel/error paths, full own AI budgets and cleanup
grace. The raw checkpoint-call fault inventory above remains open. The new frame
adds saved guest values/code at Embed attempt boundaries; measure ordinary
Embed, nested AI and deep nesting in the existing paired size/latency matrix.
This advances the caller-state and cancellation gates without closing G1–G10 or
retiring E128.

### Shared checkpoint failures and pending calls · 2026-09-08

The raw checkpoint-call inventory above is now centralized: ordinary attempt
replay, parallel prelaunch/attempt reads, loop budgets and breakpoint writes use
shared checked read/write lowering. A shared guest failure helper preserves the
storage diagnostic while resolving the active window through standard Component
Model cancellation. Successful retry checkpoint signals retain their existing
deferral; no host task interface, new Store or product flag is introduced.

Six composed regression groups cover failed reads/writes, one-shot prelaunch
faults, breakpoints, queued parallel calls and live sibling HTTP headers/body
cleanup before failure reporting. See AUDIT-14 and the active implementation
record. Retrying Split items still fall back to sequential execution; this does
not qualify concurrent retries or selective cancellation with a surviving peer.

Keep malformed stored attempt payloads, non-checkpoint preparation errors,
deeper mixed nesting, cleanup grace and G1–G10 open. Include the additional shared
function and checked call sites in the paired artifact-size/latency comparison;
there is no new WIT import, agent binary change or per-Agent wrapper.


### Parallel Agent budgets and scheduler error unwind · 2026-09-08

Private emitted timeout coverage now exercises actual parallel windows, rather
than the former timed-Agent sequential fallback. Both branch execution paths and
Split reuse per-slot budgets and one nearest-deadline standard timer. A timed
call can resolve without cancelling its unrelated pending peer; the fast branch
can advance several calls while the timed call waits. Ordinary scheduler failures
now reach their enclosing handler and resolve pending peers before that handler
runs. A Split fixture also delays one item’s connection preparation, then verifies
that it expires while the later item succeeds after observing its socket close.
AUDIT-15 lists the six composed regression groups and their limits.

Keep G5/G6 and E128 open. Next qualify timeouts during pending preparation,
including an already-launched Agent expiring while a later Agent's connection
lookup is still pending. That wait must service the earlier invocation's budget
without cancelling the later invocation. Also qualify root/parent/completion
races, nested handled exits, concurrent retries and independent cleanup grace. The shared helper state grows
by three i32 values and each existing parallel slot by 32 bytes; include untimed
workflows as well as timed and nested workloads in the paired artifact-size,
single-step, full-run, memory and soak comparisons. No new WIT import, agent
binary, host registry or product flag is introduced by this emitter change.


### Service earlier deadlines during preparation · 2026-09-08

The pending-preparation overlap identified above now has a composed reproduction
and a shared-wait fix. An earlier Agent can time out or return while the next
connection lookup is pending. Its outcome is retained in its original slot;
the lookup continues. Root cancellation and an enclosing Split timeout resolve
both kinds of call before acknowledgement/reporting. AUDIT-16 lists four new
regressions covering headers/body and cleanup while the Store remains alive.

Await shares the active window's waitable set without transferring peer handles
or allocating a separate preparation set. The window retains ownership of that
set. Await shares the existing nearest-deadline selection and slot event handling. This uses the same seven emitted helpers and
19 state parameters; the cache marker advances to `shared-v14`. Measure the extra
window scans and helper bytes in the existing paired benchmarks, including
connection-heavy windows and untimed controls.

Keep G5/G6 and E128 open for simultaneous readiness/expiry races, deeper mixed
handled exits, concurrent retries and independent cleanup grace. The full G1–G10,
publication, compatibility-retirement, Linux/soak/performance and PR requirements
remain unchanged.
