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
