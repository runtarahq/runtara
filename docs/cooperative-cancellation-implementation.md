# Cooperative cancellation implementation record

Status: cooperative root cancellation and guest deadline implementation in progress, 2026-09-08. Governing contract:
[cooperative cancellation plan](selective-isolation-plan.md). Update the existing
implementation directly; no new product feature flags or alternate backend.
All 27 built-in Agent exports now use a shared callback-binding macro. Eighteen
I/O-capable Agents await cancellable operations; the nine CPU-oriented Agents
still require cooperation-point qualification. Non-durable workflow-agents now
propagate parent cancellation through sequential/parallel calls and local retry
waits. Emitted While and sequential Split loops cooperate between iterations;
inline Embed/While/parallel cancellation has additional execution coverage.
Durable nested suspension, timeouts and the remaining plan gates are still
incomplete.

The server's Stop/cancel methods now use the environment Stop handler to deliver
lifecycle Cancel and arm an independent whole-run abort grace. Normally composed
HTTP workflows complete guest cancellation through that handler and real
PostgreSQL; CPU loops and infinite initializers escalate without fabricated
cleanup receipts. A standard cancellation callback that deliberately stalls cleanup also escalates
through grace without a false acknowledgement. Full authenticated HTTP-server
E2E, multi-owner routing and remaining P3 timeout/race work are still open.
Lifecycle acknowledgements and post-run fallback preserve accepted terminal
outcomes.

## Continuous enclosing-scope alarms · 2026-09-08

The effective enclosing deadline now owns an alarm throughout its active scope,
including assembly and checkpoint work with no pending Agent. Entering an earlier
nested scope replaces that alarm; restoring the parent restores its original
deadline and remaining grace. An unchanged owner keeps the same alarm. There are
no heap frames, saved native-handle stacks, new host interfaces or Agent wrappers.

Restoration calculates `max(0, saturating_add(budget, grace) - elapsed)` from the
original monotonic start. It does not clamp the ordinary remaining budget and
then add a fresh grace. Shared ABI exits dispose the alarm on completion, failure
and suspension. Root/parent cancellation relinquishes it before nested cleanup,
so the initiating cancellation's grace governs. Existing per-call and per-wait
alarms retain their ownership.

The seven shared helpers now exchange 21 i32 state values. The entry frame adds
one i32 scope-alarm handle and two i64 scratch locals; parallel slots remain 208
bytes. The cache tag advances to `cooperative-waits=shared-v17`. Timed Agent-free
graphs now import the existing timer interface. Zero disables a While/Split
budget and does not add timer/clock imports solely for that disabled budget.
Agent/Embed zero-timeout semantics remain unchanged.

The initial blocked-checkpoint regression failed with the independent ten-second
run timeout. With scope ownership implemented, While and Split abort after their
200 ms budget plus cleanup grace. Four nested While/Split combinations finish the
child and block the parent's completion checkpoint; the restored 1.5-second
parent budget governs, rather than the child's former alarm. Long untimed HTTP
continuations pass after normal scope completion, ordinary errors and actual
timeouts, requiring the expected route/output. Deterministic emitted-code tests
cover overdue-parent arithmetic, zero/maximum bounds and scope-alarm disposal
before root/parent cleanup. A compiled-artifact check covers disabled-loop imports.

Validation: 598 default-feature library tests passed on the final source. The
feature-gated library passed 670 tests (444.75s), followed by separate passing
runs of the expanded continuation cases (40.38s) and new disabled-import test
(0.25s); the current library has 671 tests. The full workflow execution suite
passed 395 tests (903.51s), with three manual benchmarks ignored. Feature-gated
all-target Clippy, formatting and diff checks passed. Components and database
lifecycle tests were not rebuilt/rerun in this compiler-only stage; prior-stage
results remain recorded below. No new performance or capacity result is claimed.

Full release qualification and E128 removal remain pending; this section records
the scope-ownership implementation, not completion of G1–G10 or performance,
Linux/soak and authenticated server E2E acceptance.

## Parallel call alarm ownership · 2026-09-08

Scheduler, wavefront and parallel Split launches now arm a cleanup alarm before
entering each Agent. The shared deadline selector supplies the earliest own or
inherited remaining budget; the shared alarm emitter adds the existing five-second
grace. A pending call retains its alarm while other calls or connection lookups
run. Eager return, observed asynchronous return and completed timeout cleanup
cancel/drop that call's alarm. Root and parent cancellation relinquish the window's
alarms before cleaning up calls, preserving the initiating owner's grace.

The handle occupies four bytes of existing slot padding at offset 204. Slot
stride remains 208 bytes, and the seven shared helpers still exchange 20 i32
state values. Reusing a slot with a live alarm traps instead of orphaning it.
The cache identity advances to `cooperative-waits=shared-v16`. Agent binaries,
macro expansion and native interfaces are unchanged by this stage.

Six composed test groups exercise CPU-bound entry and cancellation callbacks
under both own Agent and inherited enclosing deadlines in all three schedulers
(12 executions), plus a timed HTTP call returning while an untimed sibling's I/O
or connection lookup stays live for six seconds in the same Store. The success
cases require the normal output, so timeout recovery cannot make them pass.
Removing returned-call alarm disposal in a temporary negative control causes
`CleanupAborted` at 5.505s during the pending lookup. Restoring disposal passes
both success cases (13.80s). The abort cases require `CleanupAborted`
after the five-second grace and before the independent run timeout, without a
cleanup acknowledgement. Deterministic shared-helper tests verify that unrelated
peer alarms remain live and that root/parent propagation disarms all call alarms
before cleanup. The fixture now zero-initializes its slot allocation, matching
the production allocator's precondition.

The full feature-gated compiler library run passed 664 tests (296.57s). The
subsequent two-case success run includes the added preparation test and the
strengthened existing peer-I/O test; the current library contains 665 tests.
The full workflow execution suite passed 395 tests (810.32s), with its three
manual benchmarks ignored. Feature-gated workflow Clippy, formatting and diff
checks passed. The standard component build completed all 27 Agents and both
shared components; all 58 WASM/metadata files remained byte-identical.
All 92 component-cancellation tests passed on those outputs (79.31s). The
interactive audit page's inline JavaScript parsed successfully after its text
update. No database schema or persistence implementation changed in this stage.

Commands used with the pinned toolchain, `RUSTC_WRAPPER=`, `SQLX_OFFLINE=true`,
`CARGO_BUILD_JOBS=4`, isolated native/component target directories and the same
component outputs throughout the execution checks:

```sh
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib returned_parallel_call_disarms_alarm -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=1
cargo clippy -p runtara-workflows --features direct-wasm-integration-tests --all-targets -- -D warnings
scripts/build-agent-components.sh
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=1
```

At this stage continuous enclosing inline-scope ownership remained necessary across assembly,
checkpoint and other dispatch boundaries. These per-call tests do not establish
complete scope grace or remove E128. Paired size/latency/native-memory measurements,
Linux/soak and authenticated server E2E remain open.

## Generated deadline-wait alarms · 2026-09-08

The shared deadline-wait emitter now arms `abort-after` before sequential Agent
entry, connection preparation and enclosing-scope window waits. Its duration is
the original deadline's remaining monotonic budget plus five seconds, saturating
at `u64::MAX`. Five seconds matches the current default Stop grace. The alarm
remains live throughout timeout cleanup and is cancelled/dropped after successful
resolution. Root Cancel and parent cancellation dispose of the local alarm
before propagating cleanup, preserving the initiating owner's grace.

This extends the existing seven shared helpers with one i32 alarm handle (20
state values). The compiler timer contract imports `abort-after`; the cache key
advances to `cooperative-waits=shared-v15`. Agent binaries and their macro remain
unchanged. `wat` is a test-only dependency for deliberately uncooperative Agent
fixtures, not a production alternate compiler or backend.

The new composed emitter tests prove whole-run abort for an Agent that loops
forever on entry and an Agent whose standard cancellation callback loops forever.
Both use a 100 ms step budget and the real five-second grace, and must return
`CleanupAborted` before the separate ten-second whole-run timeout. Another test
completes a timed HTTP call and then spends six seconds in untimed HTTP work in
the same Store, proving that the disposed alarm cannot abort later execution.
Shared-helper event tests check alarm disposal after own/window timeout cleanup,
after success, and before root/parent propagation.

The 654 compiler library tests passed serially (213.01s); after adding the three
composed cases, those three passed (17.89s). Feature-gated workflow Clippy passed.
The full workflow execution suite passed: 395 tests, three manual benchmarks
ignored (720.48s). The standard build script then rebuilt all 27 Agents and two
shared components; all 58 WASM/metadata files are byte-identical to the tested
artifacts. All 92 component-cancellation tests passed on those outputs (62.98s). Performance, Linux/soak and authenticated server E2E remain
unqualified.

Commands used with the pinned toolchain, `RUSTC_WRAPPER=`, `SQLX_OFFLINE=true`,
`CARGO_BUILD_JOBS=4`, the isolated native target and the worktree's component
output directory:

```sh
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib agent_deadline_tests::cleanup -- --test-threads=1 --nocapture
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=1
cargo clippy -p runtara-workflows --features direct-wasm-integration-tests --all-targets -- -D warnings
scripts/build-agent-components.sh
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=1
```

At this stage timed-scope ownership was partial: parallel launches needed alarms held
by each pending call before entry and while other calls are prepared. Enclosing
inline scopes need continuous coverage across dispatch/assembly boundaries,
including propagation from a timed preparation wait into window-wide cleanup.
E128 and the remaining G1–G10 gates stay open. No opt-in product feature or host
workflow task registry has been introduced.

## Native cleanup alarm implementation · 2026-09-08

The existing host timer interface now implements `abort-after(ms)`. This is an
explicit platform emergency timer; the standard Component Model subtask handle
owns its lifetime. The host receives a duration and no scope identifiers. An
independent Tokio timer latches whole-execution abort, and standard subtask
cancellation disarms it after guest cleanup. A disposal guard synchronizes with
expiry to prevent stale alarms from aborting later work. There is one shared
abort latch/notification per Store and one native timer task per armed alarm.

The production executor consumes the latch both at epoch checks and while blocked
in host I/O. A late successful return cannot override an already latched abort.
The shared runtime host accessor also rejects new runtime calls after expiry,
including terminal writes and acknowledgements. This does not roll back calls
that began before expiry.
`CleanupAborted` distinguishes this exit from normal timeout and cooperative
cancellation. Environment records failure, or cancellation if Cancel is pending,
with termination reason `aborted`; it does not acknowledge the command and uses
the existing running-state guard to preserve accepted terminal outcomes. The
old task-outcome compatibility adapter exposes this as a trap, not a fabricated
successful cancellation.

Five tests use the production executor, current synchronous cancellation ABI and
a composed callback Agent without runtime imports: CPU cleanup, pending-I/O
cleanup, CPU work before the parent regains control, continued execution after
disarming, and a late-success race. Two more exercise command and dispatcher
entry points. An additional test covers complete/fail in both supported runtime
versions with and without expiry, using the ordinary executor. It fails without
the shared runtime guard because native return rejection alone permits a late
publication. Five native alarm tests cover expiry without polling, unpolled disposal, overlapping
alarms, zero/maximum grace and 1,000 disposals. Two real database tests cover
terminal recording and preservation. A negative Component Model control proves
that an ordinary timer future throwing an error cannot interrupt CPU cleanup.

At the native prerequisite stage, the emitter did not yet arm this alarm. A timed scope must arm it before
entering potentially noncooperative code, for the remaining deadline plus grace,
and retain it through cancellation cleanup. Integrate this into the shared
helpers while preserving initiating-owner grace and externally configured
root Stop deadlines; do not reset or shorten a propagated cancellation budget.
No optional async-cancel feature, per-Agent wrapper, graph registry, extra Store,
or product flag was added. Native blocking code without a cooperation point is
still outside this proof. AUDIT-18 and the governing plan retain those limits.

Validation: **124 native host tests** (1.86s) passed after the runtime-boundary
guard; the focused test was also run without that guard and failed on a late
publication. Before that final guard, **92 component-cancellation tests**
(62.77s), **395 workflow execution tests** (534.55s; three manual benchmarks
ignored), and both new persistence tests (0.88s) passed. The database tests used
a separate local PostgreSQL container/database; that container was stopped after
testing. Feature-gated host/environment Clippy passed. No Agent/guest WIT changed,
so these runs reused the previously built component artifacts. No current-stage
paired performance report, Linux/capacity soak or authenticated server E2E result
is claimed.

## Async-cancel grace qualification · 2026-09-08

Five composed tests in `cooperative_cancellation/async_cancel_grace.rs` qualify
the optional standard `canon subtask.cancel async` operation on Wasmtime 46.0.1.
The production engine rejects this ABI today. Only the test engine enables
`wasm_component_model_more_async_builtins`; no product selector is introduced.

While cleanup awaits I/O, async cancel returns `BLOCKED`, and the parent can wait
on both the original subtask and the existing timer import. Cleanup can either
acknowledge cancellation or return normally. Both cases are exercised twice in
the same instance. A stalled cleanup lets the parent observe grace expiry; the
fixture deliberately traps rather than report successful cleanup. Pending I/O
is disposed when the whole Store is destroyed.

A callback entering an infinite CPU loop prevents even async cancel from
returning. The proof observes repeated epoch yields to the host but no parent
continuation or guest grace selection. The independent epoch watchdog interrupts
the entire run. This matches the pinned engine's `subtask_cancel` implementation:
it yields to a cancellable callee before checking whether to return `BLOCKED`.

The extension therefore supplies an I/O cleanup wait, not a complete bounded
grace implementation. Production adoption still needs independent whole-run
watchdog coverage for scoped deadlines, including nested and runtime-free
published workflows. Existing root Stop already arms that watchdog externally;
this does not establish scoped timeout coverage. No emitter, guest binding,
production engine, WIT, cache identity or E128 behavior changes in this proof.

Validation: all five focused cases passed; the full component-cancellation suite
passed **91 tests** in 64.17s. The tests assert exact trace order, cancellation
resolution, trap type, resource disposal and repeated instance use. After changing
the success cases to cancel a generous timer instead of racing a short deadline,
all five focused tests passed again in 0.64s. This also verifies asynchronous
cancellation and resolution of the pending host timer. Feature-gated component
host Clippy, formatting, diff whitespace and HTML JavaScript syntax checks passed.
No agent
rebuild, workflow execution suite, server E2E, Linux/soak or performance comparison
was run for this test-only qualification. AUDIT-17 records the cases and limits.

## P0: initial ABI inventory

The following records the starting state before the HTTP binding update below
(the standards proof was committed as `5c8863f7`):

- Wasmtime 46.0.1 has Component Model async enabled in the host dependency. The
  proof uses the existing engine builder, without optional async extensions,
  custom task imports or isolated execution.
- `runtara:agent/capabilities.invoke` and workflow lifecycle `invoke` are
  async-typed. The HTTP agent uses `wit_bindgen::generate! { async: false }`;
  the workflow lifecycle explicitly documents its synchronous lift.
- `runtara-http/src/host_io.rs` similarly synchronously lowers the async-typed
  request import. Its concurrent host implementation permits overlap, but that
  does not make the blocked guest call acknowledge cancellation.
- The resolved guest binding generator is wit-bindgen 0.58. Its standard async
  support receives cancellation event 6 and destroys the guest future, invoking
  destructors and standard subtask cancellation. Switching export typing alone
  does not select that implementation.
- Generated parallel drains already use waitable sets and poll lifecycle signals,
  but an operation must wake the drain before polling can observe a newly arrived
  signal. Signal readiness/timed polling must participate in the wait itself.
- `RuntimeHost::is_cancelled` acknowledges the existing root Cancel when observed;
  the new path must distinguish that from cleanup/terminal publication as the
  plan requires. No new signal transport or acknowledgement changes are made yet.

No feasibility result below is a production performance baseline. The historical
measurements remain linked from the plan; fresh interim pairs are recorded later
in this document and do not close the performance qualification gate.

## P1: standard composed-component proof

Added the automatically discovered integration target
[`cooperative_cancellation.rs`](../crates/runtara-component-host/tests/cooperative_cancellation.rs)
with two handwritten WAT fixtures. It runs one standard composed component in one
Store, with parent workflow and two agent instances composed as peers. It uses
standard async calls, waitable sets, `subtask.cancel`, `subtask.drop` and
`task.cancel`. There is no package catalog, task resource, launcher, invocation
ledger or per-invocation Store.

The signal import is a test readiness gate, not the production lifecycle signal
interface. The I/O import is a test transport, not a built HTTP agent. These
boundaries are deliberate and remain outstanding for full P1 completion.

| Test | Evidence |
|---|---|
| `standard_cancellation_preserves_composed_sibling_and_instance_state` | Both operations start before the signal; parent WASM requests cancellation; native I/O future destruction precedes guest acknowledgement, which precedes parent continuation. Sibling finishes and the same cancelled agent instance is invoked again with its global call counter preserved. |
| `callee_can_return_a_value_instead_of_acknowledging_cancellation` | Callee cleans up but returns normally; parent receives the standard returned state and checks the result instead of assuming a cancellation outcome. |
| `standard_cancellation_closes_pending_http_headers` | Controlled loopback endpoint withholds headers; standard cancellation drops the pending request and the endpoint observes connection closure. |
| `standard_cancellation_closes_pending_http_body` | Endpoint sends headers and an incomplete body; cancellation closes the connection before the response finishes. |
| `synchronous_io_lowering_without_cooperation_does_not_acknowledge_cancellation` | Same parent/control path with the existing agent ABI shape remains unresolved until the test watchdog. A trap or invalid fixture does not count as the expected result. |

The positive tests also require reusing the original agent instance successfully;
throwing away that instance would fail the global-state assertion. Trace assertions
check the actual order of I/O destruction, guest acknowledgement and parent
continuation. The server never supplies a full response to make a cancellation
case pass. Test server tasks are owned and cleaned up on errors.

Two ABI constraints were established while constructing the proof:

1. Standard composition connects peer components. A core caller in an ancestor
   component cannot use an adapter to reenter its nested descendant; the first
   fixture arrangement correctly trapped. The final fixture follows the normal
   sibling-component composition shape.
2. Before synchronous subtask cancellation, remove the subtask from its waitable
   set with `waitable.join(handle, 0)`. Otherwise the pinned runtime traps because
   a waitable cannot be used synchronously while it belongs to a set. Wait for
   resolution before dropping the subtask and its set.

## Verification and remaining implementation

All five integration tests passed against Wasmtime 46.0.1, including the
controlled HTTP header/body cases and the synchronous-binding negative case.
`cargo fmt --all -- --check`, `git diff --check`, and
`cargo clippy -p runtara-component-host --all-targets -- -D warnings` passed. The test command was
`cargo test -p runtara-component-host --test cooperative_cancellation`, with
`RUSTC_WRAPPER=`, `SQLX_OFFLINE=true`, and the existing isolated target directory.
No built Agent, DSL, database or server E2E qualification was claimed or run in
this proof; guest production sources and WIT have not changed.

Next implementation: expose a cancellation-capable async HTTP transport/agent
binding using the existing request interface and preserve request shaping,
connection proxy behavior and error coercion. Prove the built agent in standard
composition, then wire emitted workflow waits to existing lifecycle signals.
Use a separately built baseline revision for differential qualification, without
adding a flag-selected production path.

P1 remains incomplete until the built HTTP agent and emitted DSL pass; P0 still
needs the fresh baseline and experimental-artifact usage inventory. The complete
construct parity, persistence/lifecycle races, timeout integration, emergency
abort qualification, size/timing comparison, Linux capacity and local-server E2E
gates remain pending. No production code or default was changed by this proof.

## Existing HTTP agent: standard async bindings

The normal `runtara-agent-http` WASM export now uses wit-bindgen's async callback
ABI and awaits `runtara-http::RequestBuilder::call_agent_async`. The normal
component build produces this implementation; there is no opt-in flag or separate
candidate artifact. The native synchronous capability remains available for its
existing callers and metadata executor. Other agents' synchronous paths have not
yet been migrated, and are not claimed to acknowledge cooperative cancellation.

The HTTP library provides standard async bindings for its existing
`runtara:host-io/http.request` import. Request encoding, response decoding, proxy
request construction and proxy response handling are shared with the existing
blocking API. The WIT is now a source file used by both binding generations;
wit-bindgen's documented `type_section_suffix` prevents their compile-time type
metadata from colliding. This introduces no runtime task interface, host task
registry or custom artifact package. The host I/O implementation is unchanged.

Added real-agent tests under the existing `component-integration-tests` gate,
using the normal `RUNTARA_AGENT_COMPONENTS_DIR` bundle. The existing CI component
suite discovers these tests without a new test feature or workflow selector.
The fixture uses wac-graph to compose the real HTTP agent with a WASM parent and
runs the normal host linker/I/O. Its signal source is still a test gate.

- Withheld response headers: parent WASM cancels the real Agent subtask; the
  endpoint observes connection closure, an independent pending operation finishes,
  and parent WASM successfully invokes HTTP again in the same Agent instance.
- Partial response: the endpoint sends headers and an incomplete body before the
  cancellation signal; the same closure/reuse assertions pass. This does not
  independently observe the exact host body-read phase. The earlier transport
  fixture proves cancellation after entering body consumption; a phase-observed
  production body-read case remains part of G3 qualification.
- Direct async export: malformed JSON, unknown capability and invalid typed input
  preserve permanent error codes/severity/retryability and decoding precedence.
- Proxy/coercion: string timeout/bool inputs are coerced; tenant, connection and
  endpoint context and escaped query parameters reach the existing proxy;
  control headers are excluded from forwarded headers; a proxied 503 with
  `fail_on_error=false` preserves response body, headers and success=false.

Verified so far for this update:

- `scripts/build-agent-components.sh`: all 27 Agent components plus stdlib/runtime
  and their generated metadata rebuilt through the normal path.
- `cargo test -p runtara-http --features native`: 12 tests passed.
- `cargo test -p runtara-agent-http`: 16 tests passed.
- `cargo test -p runtara-component-host --features component-integration-tests,isolated-step-poc --tests`:
  145 tests passed, one pre-existing manual benchmark ignored. This run contained
  the five primitive cases and two built-agent cancellation cases.
- Subsequent focused real-agent run: all four real-agent cases passed, including
  the two new input/proxy compatibility tests (147 distinct host tests across the
  broad and focused runs).
- Native affected-crate all-target Clippy, including all four real-agent tests,
  and WASM-target HTTP Agent Clippy passed with `-D warnings`. Formatting and
  diff whitespace checks also passed.
- `cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute`:
  250 tests passed; the two existing manual performance benchmarks were ignored.
  This covers emitted workflow compatibility with the normally built HTTP Agent,
  including audit fixtures, parallel HTTP overlap and pause behavior.

No emitted cancellation selection, lifecycle acknowledgement changes, cooperative deadline
handling, emergency grace integration, production rollout qualification or fresh
size/latency comparison is established by these Agent-level tests.

## Lifecycle signal observation before cleanup

Runtime 0.4.0 adds `poll-signal`, a read-only view of the existing lifecycle
command, with its type, command ID and payload. Both the native persistence
runtime and the guest SDK runtime implement it. It does not acknowledge the
command, set the local cancelled latch, change instance status, consume a custom
signal, or create a checkpoint. Existing decorators forward the read without
introducing execution bookkeeping.

The guest must retain an observed command while cleaning up active subtasks;
rate limiting can make a subsequent poll return none. Once cleanup succeeds it
can use the existing `handle-checkpoint-signal` receipt, which identifies the
exact command. A superseded receipt must not consume its replacement. After a
durable sleep interrupted by cancellation, the new poll also clears the legacy
ignored-signal escalation marker, allowing cleanup calls without prematurely
publishing cancellation.

Normal new compilations and the shared runtime build use 0.4.0. The host keeps
the 0.3.0 interface registered for already-built artifacts, with its existing
consuming helpers. This is artifact compatibility, not an opt-in backend. A
linker test verifies both interface shapes and rejects the new poll on 0.3.0.
Existing 0.3.0 WAT execution fixtures remain unchanged.

New persistence tests verify repeated observation before acknowledgement for
Cancel/Pause/Shutdown, unchanged running status and custom payloads, duplicate
receipts, Pause superseded by Cancel, and observation after interrupted sleep.
The full native runtime-host database module passed: 26 tests against the
isolated Postgres fixture. The normal component build also passed for all 27
agents and both shared components.

The 573 compiler unit tests and 148 component-host tests passed; the latter
include the nine standard/real-HTTP cancellation cases and retained 0.3.0
execution fixtures. One existing manual host benchmark was ignored. The
standalone runtime and WIT suites passed with 9 and 31 tests respectively.
Commands used the existing integration test features; no new gate was added.
The 46 existing scoped-runtime database tests also passed, including old runtime
imports and signal receipt behavior. The isolated database container was stopped
after verification.

Full emitted-workflow regression passed again: 250 tests, with the two existing
manual performance benchmarks ignored. All-target Clippy for the five affected
crates passed with their component, database and emitted-workflow integration
features enabled, as did formatting and diff whitespace checks. The main commands
were:

```sh
cargo test -p runtara-workflow-wit -p runtara-workflow-runtime -p runtara-component-host --lib
cargo test -p runtara-workflows --lib
cargo test -p runtara-component-host --features component-integration-tests,isolated-step-poc --tests
cargo test -p runtara-environment --features db-integration-tests --lib runtime_host::tests
cargo test -p runtara-environment --features db-integration-tests --lib runtime_host::scoped::
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute
cargo clippy -p runtara-component-host -p runtara-environment -p runtara-workflows -p runtara-workflow-runtime -p runtara-workflow-wit --all-targets --features runtara-component-host/component-integration-tests,runtara-environment/db-integration-tests,runtara-workflows/direct-wasm-integration-tests -- -D warnings
```

The database helper supplied only the isolated fixture's URL. Native checks used
`SQLX_OFFLINE=true`, an empty `RUSTC_WRAPPER`, and the existing isolated native
target directory; integration tests used the normal release component bundle.

This establishes the signal transport needed by emitted cancellable waits. The
emitter does not call `poll-signal` yet. Its waits still need timer readiness,
standard cancellation of every active subtask in the selected scope, retained
command handling and acknowledgement after cleanup. Whole-run emergency grace
and terminal publication races remain unqualified. In particular, the existing
core acknowledgement policy can still apply Cancel to an already-terminal
instance; the plan's accepted-completion rule needs explicit implementation and
compatibility tests. No claim of completed G4/G7 is made by read-only observation.

## Sequential generated waits

The direct emitter now lowers sequential Agent calls asynchronously, including
auxiliary AiAgent calls that use the same invocation helper. It emits standard
waitable sets, `subtask.cancel` and `subtask.drop` into the normal composed
artifact. The existing instance pools still determine component multiplicity.
No new selector, task resource, per-call Store or isolated package is involved.
Pure workflow-agent artifacts that omit the runtime keep that mode; they can
await component completion without importing a lifecycle polling timer.

For runtime-backed sequential calls, WASM polls before launch and includes the
existing concurrent clock import in its wait. Polls/heartbeats recur on a
one-second timer while a call remains pending. Agent results at address 0 and
signal/error results at 128 have separate scratch regions, so async completion
cannot corrupt signal observation. Canonical discriminants are read as bytes,
without assuming their padding is zero. The pending wait handles use guest
locals; strings returned by runtime calls still use canonical ABI allocation.

A completion event accepted by the wait exits without another signal poll. Once
WASM observes root Cancel, it detaches the Agent handle, requests standard
cancellation, waits for resolution, drops the handle/set and acknowledges the
exact command. The callee may resolve as returned or cancelled; either resolution
releases the subtask, while the selected root intent stops ordinary execution.
The generated early return bypasses automatic retries and step `onError` paths.
A rejected acknowledgement traps rather than reporting completed cancellation.
Signal/heartbeat errors also clean up the pending call before returning failure.
Synchronous cancellation may wait indefinitely for an uncooperative callee;
whole-run emergency supervision is still required.

Added [`cooperative_workflow_cancellation`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/mod.rs)
to the existing emitted-workflow integration suite:

| Test | Evidence |
|---|---|
| `emitted_cancel_before_agent_launch_does_not_send_http` | A pre-existing root intent is acknowledged without sending an HTTP request. |
| `emitted_cancel_interrupts_pending_headers_without_retry_or_recovery` | The endpoint never supplies headers; generated WASM stops HTTP and the endpoint observes closure before acknowledgement publication. |
| `emitted_cancel_interrupts_partial_body_without_retry_or_recovery` | The endpoint sends headers and an incomplete body, then requests cancellation; closure precedes acknowledgement. This does not independently observe the exact host body-read phase. |
| `emitted_signal_poll_failure_cleans_up_http_before_failing` | A signal transport error after HTTP starts closes the pending connection, preserves the error and does not acknowledge a command or enter recovery. |

The fixtures use the normally built HTTP Agent with a five-minute capability
timeout and a ten-second whole-run test watchdog. The endpoint cannot complete
the blocked response. Retries and `onError` are enabled to catch unintended
continuation. Artifact checks require no scoped Agent adapters, invocation
inventory or superseded tasks import. The signal source is a controlled
`RuntimeHost`; this is emitted DSL plus real host I/O, not API/server E2E.

Verification: 573 compiler unit tests passed. The full emitted suite passed
253 tests with two pre-existing manual benchmarks ignored; a subsequent focused
run passed all four new cases, including the added transport-error case (254
distinct integration tests across the two runs). Affected-crate all-target
Clippy with `direct-wasm-integration-tests` and `-D warnings` passed. Compiler
inspection assertions now follow the async invoke, and the replay control-flow
inspection counts nested blocks/loops so it still proves cache hits skip calls
and fresh results checkpoint after invocation. Guest sources/WIT were unchanged
in this stage; tests used the normal release components built in the preceding
stage. Commands:

```sh
cargo test -p runtara-workflows --lib
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute cooperative_workflow_cancellation
cargo clippy -p runtara-workflows --all-targets --features direct-wasm-integration-tests -- -D warnings
```

Remaining at the sequential stage: replace the parallel drains/scheduler's consuming signal checks and
cancel every active peer before root acknowledgement; qualify sequential waits
inside those schedulers, pause/shutdown observation and superseding commands;
migrate the other Agent/export bindings and compiled workflow-agent cancellation
delivery; implement deadline selection, independent emergency grace and terminal
race rules; then complete persistence/server E2E, size/timing and capacity gates.
Sequential helpers do not establish cancellation of parallel or nested scopes.


## Parallel generated waits and deferred acknowledgement

The normal Split window, branch scheduler and depth-wavefront now use the same
read-only lifecycle observation and standard Component Model cancellation path.
The compiler stores standard call handles in the existing guest result slots;
it adds no host task registry, per-call Store, task interface or product flag.
The existing normal component bundle and instance pools are used unchanged.

Each active window includes a one-second polling timer in its waitable set. Timer
completion only triggers another observation; it cannot decrement the active
Agent count or advance a branch. Returned handles are removed from their slots
before another call can reuse those handles. On root Cancel, WASM detaches,
cancels and drops every retained call and polling timer, drops the waitable set,
and only then acknowledges the exact lifecycle command. Sequential invocation
helpers also close an active peer window before their cancellation/error return.
Signal/heartbeat failures use the same cleanup before reporting failure.

Checkpoint responses use this handling too: a completed branch may save a
checkpoint while another branch is blocked. A Cancel carried by that response
must clean the pending sibling before acknowledgement; the saved checkpoint
remains intact. Pause and shutdown are retained in guest locals until completed
results have been assembled and checkpointed at the window boundary. A later
rate-limited `None` observation does not erase the receipt. A newer Cancel can
supersede it. Nested assembly deferral uses a depth counter so an inner boundary
cannot release an outer boundary's deferred acknowledgement. The shared
checkpoint helper's consuming polls run only after that deferral ends; other
composite/loop boundaries still require broader qualification.

Additional emitted DSL tests (normal HTTP Agent and native host I/O):

| Test | Evidence |
|---|---|
| `emitted_cancel_cleans_every_parallel_split_call` | Both requests remain hung; both connections close before the command is acknowledged. |
| `emitted_cancel_cleans_every_scheduled_branch` | The independent branch scheduler closes every pending peer before acknowledgement. |
| `emitted_cancel_cleans_every_wavefront_branch` | Two branches containing later waits exercise the wavefront path; cancellation closes both initial calls. |
| `emitted_signal_poll_failure_cleans_every_parallel_call` | A failed signal read closes every pending call before workflow failure. |
| `emitted_checkpoint_cancel_cleans_pending_sibling_before_ack` | One call completes and checkpoints; its returned Cancel command cleans the still-hung sibling before acknowledgement. |
| `emitted_pause_observed_once_checkpoints_every_sibling_before_ack` | A single observation survives subsequent `None` reads; both sibling checkpoints precede acknowledgement and replay sends no new HTTP requests. |
| `emitted_shutdown_observed_once_checkpoints_every_sibling_before_ack` | Shutdown has the same drain/checkpoint/replay ordering without becoming Cancel. |
| `emitted_cancel_supersedes_pause_while_parallel_calls_hang` | A retained Pause does not hide a newer Cancel; both hung calls are cleaned and only Cancel is acknowledged. |

These are controlled-runtime integration tests, not public API/Core/server E2E.
The ten-second test watchdog is a failure bound, not the cancellation mechanism;
all runs use `cancel: None`. Only the HTTP Agent currently has qualified
cancellation-capable bindings. Other Agents and compiled workflow-agents can
still delay standard synchronous cancellation indefinitely.

The cancellation result check accepts all three terminal resolutions: returned
(2), cancelled before entry (3), and cancelled after entry (4). The added standard
Component Model proof
`cancelling_a_queued_call_resolves_before_entry_and_can_be_dropped` applies
backpressure to queue a second call, cancels and drops it before Agent entry,
and verifies unchanged sibling behavior and reuse of the Agent instance. This
is a standard-mechanism proof; the emitted HTTP cases exercise entered calls.

Performance remains unmeasured for this stage. Returned-handle removal currently
scans the window's slot array, so a drain of K calls can perform O(K²) comparisons;
this must be measured and addressed before claiming scalable unlimited windows.
Polling and emitted cleanup instructions also add work and binary bytes. The
planned revision-to-revision measurements must quantify those costs; the older
isolated-execution reports are not evidence for this implementation.

Remaining gates include nested workflow-agent delivery, other Agent bindings,
sequential fallback/retry and wider pause/shutdown qualification, cooperative
step deadlines, independent emergency grace, accepted-completion races,
server/persistence E2E and the complete size/timing/capacity comparison. E128
Agent/Embed timeout rejection remains in place.


Verification for this stage:

- 573 compiler unit tests passed.
- The full emitted-workflow suite passed 262 tests with four test threads; the
  two existing manual performance benchmarks remained ignored. The initial
  default-concurrency run passed 258 tests but hit the five-second final-join
  watchdog in four historical isolation cancellation tests. Those four passed
  in a focused run and in the full four-thread run. No assertions, deadlines or
  production settings were relaxed.
- After adding the pre-entry cancellation result, the 12 focused emitted
  cancellation tests and all 573 compiler unit tests passed again.
- All 10 standard cancellation/real HTTP component proofs passed, including
  queued-call cancellation and the negative synchronous-binding test.
- All-target Clippy for the compiler and component host, with their existing
  integration test features and `-D warnings`, passed. Formatting and diff
  whitespace checks passed.

```sh
cargo test -p runtara-workflows --lib
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute cooperative_workflow_cancellation
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation
cargo clippy -p runtara-workflows -p runtara-component-host --all-targets --features runtara-workflows/direct-wasm-integration-tests,runtara-component-host/component-integration-tests -- -D warnings
```

Guest sources and WIT were unchanged; these checks used the existing normal
release bundle. No database/server E2E, release performance measurements or
Linux qualification was run in this stage.


## Shared async capability dispatch and Slack bindings

`#[capability]` now accepts authored Rust `async fn` capabilities. The generated
async dispatcher uses the same input coercion, deserialization, structured/plain
error handling and output serialization as the synchronous dispatcher. Existing
synchronous descriptors and native SFTP registry calls retain their signatures.
An async descriptor holds a function returning a standard boxed Rust future for
metadata consumers. Guest `invoke` awaits the generated function directly and
does not use that boxed adapter. There is no new executor, host task service,
workflow scheduler, product flag or backend selector.

HTTP now uses this shared macro dispatcher; its temporary handwritten async
coercion/error wrapper was removed. Slack's normal export uses the cancellation-
capable wit-bindgen callback ABI. `send-message`, `add-reaction`, and `upload-file`
await the existing host-I/O transport. Each stage of upload (obtain URL, upload
bytes, finalize) is independently awaiting I/O, so cancellation drops the future
at the current stage and prevents the following stage from starting. A fresh
invocation can reuse the same Agent instance after cancellation.

The Rust capability functions for HTTP and Slack are now async on both targets.
The existing native HTTP backend remains blocking ureq: polling the native async
facade can block and does not provide cancellable native I/O. This preserves the
native test/metadata build's transport behavior; WASM cancellation is provided
by the standard async host import. This is not a claim of new native async I/O.
All workflow tests use the built WASM components for that guarantee.

New macro tests exercise:

- async/sync metadata and input coercion parity;
- input errors, plain errors, structured errors and retry metadata;
- output serialization errors;
- destruction of a pending capability when its dispatch future is dropped.

New built-Slack tests use a local proxy stub throughout; they do not contact Slack
or send real messages. They check cancellation with missing headers and partial
proxy bodies, each of the three upload stages, cleanup before a sibling resumes,
and a successful later call in the same component instance. The next request
after upload cancellation must be the parent's new `send-message`, proving that
the cancelled upload cannot advance to another stage. The fixtures also check
connection/tenant forwarding, absence of connection injection on presigned upload
bytes, input coercion, dispatch/connection errors, HTTP 429 retry metadata and
Slack's HTTP-200 error response contract.

The emitted DSL suite adds
`emitted_cancel_interrupts_slack_without_retry_or_recovery`: a normal compiled
Slack step, with retries and `onError` present, waits on the local proxy while an
existing lifecycle Cancel reaches WASM. The test requires closure before root
acknowledgement and forbids retry/recovery/ordinary completion. Artifact checks
still require a normal composed binary with no isolation inventory or tasks import.

At the end of this stage, HTTP and Slack were migrated; the other 25 Agent
bindings, compiled workflow-agent cancellation, CPU cooperation points, deadline handling, emergency grace,
terminal races, server E2E and performance/capacity gates remain open. The full
plan has not been completed by these component tests.


Verification for the shared dispatcher/Slack stage:

- The normal build script rebuilt all 27 Agent components and both shared
  workflow components, including their generated metadata.
- Macro tests passed: 54 unit tests, the four new async contract tests and the
  existing connection-condition test. HTTP's 16 native tests, Slack's two native
  tests, the HTTP client's 12 tests and four native registry tests passed.
- DSL/compiler unit tests passed (221 + 573).
- All 17 standard/real-Agent cancellation proofs passed, including seven Slack
  cases. The production-bundle fixture harness and six dispatcher tests passed.
- The full emitted-workflow regression passed 262 tests with four test threads,
  with two existing manual benchmarks ignored. The later focused run passed all
  13 cancellation cases, including the new Slack DSL case (263 distinct emitted
  integration tests across these runs).
- Affected-crate all-target Clippy passed with the existing component/workflow
  integration features. Formatting and diff whitespace checks passed.

```sh
scripts/build-agent-components.sh
cargo test -p runtara-agent-macro --tests
cargo test -p runtara-agent-http -p runtara-agent-slack --lib
cargo test -p runtara-http --features native --lib
cargo test -p runtara-agents --test custom_module_registration_test
cargo test -p runtara-dsl -p runtara-workflows --lib
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation --test dispatcher --test capability_fixtures
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute cooperative_workflow_cancellation
```

All I/O fixtures use local endpoints. No database/server E2E, Linux qualification,
release size/timing comparison or capacity measurement was run in this stage.
Cancelling a later upload stage does not undo a URL/file allocation or bytes
already accepted by the remote service; these tests establish local workflow
interruption and resource cleanup, not external rollback.

## AI provider bindings and emitted AiAgent cancellation

OpenAI, Bedrock and AI-tools now use the same normal callback bindings and shared
async capability dispatcher as HTTP and Slack. All 23 capabilities across these
three Agents await their HTTP calls. Image, vision, structured-output, moderation,
model-listing and embedding helpers use that path too. There is no product flag,
new host task service or alternative workflow composition.

AI-tools' `chat-completion`, `chat-turn` and `summarize-memory` also depended on
`runtara-ai`'s synchronous provider abstraction. The normal Agent path now awaits
`run_completion_async` and a required `CompletionModel::completion_async` method.
The trait has no synchronous default for this method: such a fallback would hide
an uninterruptible provider. Its boxed future permits the existing dynamic
provider selection; it does not introduce an executor or host scheduling policy.

Provider selection, timeout forwarding, request preparation and response parsing
are shared with the existing synchronous entry points. Those entry points remain
for legacy/native callers, rather than becoming a selectable workflow backend.
As in the preceding stage, native HTTP remains blocking; cancellation evidence
comes from the built WASM components and production component I/O linker.

The new built-Agent tests cover 24 pending-I/O cases across OpenAI and Bedrock:
headers and partial proxy bodies for provider-specific text completion, AI-tools
text helpers, shared chat completion, chat turns, memory summaries and embeddings.
Each case checks local request closure before the parent resumes, independent
sibling completion, and a successful new call in the same Agent instance. For
Bedrock embeddings, the second batch item must never start after the first is
cancelled. The memory-summary cases must acknowledge cancellation rather than
returning the ordinary provider-error fallback state.

Additional tests preserve connection/tenant and provider forwarding, dispatch and
validation errors, the HTTP 429/403/503 contracts, retry metadata, completion text,
tool calls and usage parsing. They explicitly preserve the existing distinction
between the provider-specific HTTP errors and shared completion errors. Ordinary
summary failure still yields the existing fallback; cancellation drops the future
before that fallback can execute.

Three normal emitted-DSL tests exercise single-shot AiAgent completion, an AiAgent
tool turn, and memory summarization after a completed turn. Existing lifecycle
Cancel reaches guest-owned waits. They require I/O cleanup before acknowledgement
and reject normal completion or `onError` recovery. The summary test also rejects
memory writes after cancellation. It allows the existing object-model Agent's
memory load to finish first; this does **not** qualify cancellation during that
Agent's still-synchronous load/save I/O.

Five of the 27 built-in Agent bindings are now migrated. The other 22 bindings,
compiled workflow-agent cancellation, bounded CPU cooperation points, cooperative
timeouts, emergency grace, terminal races, server E2E, Linux qualification and
fresh release size/timing/capacity comparisons remain open. These tests do not
establish those gates or undo any external provider work.

Verification commands for this stage (the existing integration features select
only test targets):

```sh
scripts/build-agent-components.sh
cargo test -p runtara-ai -p runtara-agent-openai -p runtara-agent-bedrock -p runtara-agent-ai-tools
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
```

Results: the normal build produced all 27 Agent components and both shared
workflow components with metadata. All 54 focused native tests passed (29 shared
AI, 19 AI-tools, five Bedrock, one OpenAI); the existing ignored documentation
example remains ignored. All 25 component cancellation tests passed, including
eight new AI test functions. The full emitted-workflow suite passed 266 tests
with four test threads, including all 16 cooperative lifecycle cases. Its two
existing manual release benchmarks remain ignored and supply no new performance
evidence. Affected-crate and integration-target Clippy, formatting and diff
whitespace checks passed. No external provider calls, database/server E2E, Linux
qualification, release size/timing comparison or capacity run was performed.
The six standard proofs also passed in the default test build, without staged
Agent integration tests enabled.

## Object-model I/O and memory load/save

The normal object-model Agent now uses callback bindings and async dispatch for
all 14 capabilities. Its GET, POST and PUT helpers await the existing internal
HTTP transport. Connection IDs remain in the query string and JSON body, tenant
context remains in `X-Org-Id`, and requests still target `RUNTARA_OBJECT_MODEL_URL`
directly. The server API, SQL execution rules and storage implementation are
unchanged; no host-side workflow management was added.

Built-component tests cover 26 pending-request cases, each with no headers and
with a partial response body: SQL query/execute; memory schema lookup; optional
schema creation; loading conversation messages; save-time lookup; and memory
create/update. After cancellation, the next request must belong to a fresh SQL
query in the same Agent instance. This rules out advancing to a later memory
stage from the cancelled invocation, while the independent sibling completes.
Additional tests preserve the distinct read/write retry contracts for HTTP 429,
503 and 413. Every endpoint is a local fixture, not a database.

Four emitted-DSL cases extend the lifecycle proof to SQL query, SQL execute,
AiAgent memory load and AiAgent memory save. Root cancellation bypasses retries
and `onError`, requires local I/O cleanup before acknowledgement and does not
publish workflow completion. Cancelling memory load prevents the model call;
cancelling the save prevents subsequent workflow execution. The fixture reads
complete request bodies before responding or signalling, including bodies split
across TCP packets.

These tests qualify local workflow cancellation, not rollback of a schema change,
a SQL statement or a memory write already accepted by the server. They do not
add database transaction ownership to cancellation. Native HTTP continues to use
its existing blocking backend; the cancellation guarantees here are tested in
WASM. Six of 27 built-in Agent bindings are now migrated. The other 21 bindings
and the nested-workflow, CPU, timeout, emergency-abort, terminal-race, E2E and
performance/capacity gates remain open.

```sh
scripts/build-agent-components.sh
cargo test -p runtara-agent-object-model
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
```

Results: the normal build regenerated all 27 Agent components and both shared
workflow components with metadata. All 11 object-model native tests, all 29
component cancellation tests and all 270 emitted-workflow tests passed (four test
threads; the two existing manual benchmarks remain ignored). The emitted suite
includes all 20 cooperative lifecycle cases. Affected-crate all-target Clippy with
the existing integration test features, formatting and diff whitespace checks
passed. No database/server E2E, Linux run or fresh performance/capacity measurement
was performed in this stage.

## Mailgun, Teams and MCP I/O

Mailgun, Teams and MCP now use their normal callback exports and the shared
async capability dispatcher. Mailgun awaits its existing form submission. Teams
awaits each Bot Connector activity, so cancellation can stop text chunking after
an earlier activity has already completed. MCP awaits connection-parameter lookup,
initialization, the initialized notification and the final tools/list or tools/call
request. Its existing ephemeral session and scope rules remain guest-owned; no
host workflow task manager or product selector was added.

The component tests cover 20 blocked-request cases, each before headers and during
a partial response body: Mailgun sends, both chunks of a Teams message, MCP
connection lookup, and all three handshake stages for both search and invocation.
Each cancellation must close local I/O, permit the independent sibling to finish,
and allow a fresh invocation in the same Agent instance. A new MCP invocation
must initialize a fresh session rather than continue the cancelled handshake.
The tests check Teams endpoint references, encoded conversation paths, text/card
chunking and timeout coercion; Mailgun form fields and repeated tags; and MCP
session headers, extra headers and SSE responses. Additional cases preserve
HTTP error/retry classification, MCP protocol/server errors, forbidden-tool
rejection and successful search scope/schema handling.

Four normal emitted workflows exercise Mailgun cancellation, cancellation during
the second Teams chunk, MCP initialization, and an MCP tool request after its
handshake completed. The existing root lifecycle signal causes guest cleanup
before acknowledgement and bypasses retries, normal continuation and `onError`.
Earlier completed requests remain completed; none of these tests claims to undo
an accepted email/activity, an allocated remote MCP session or remote tool work.
All endpoints are local fixtures; no real message or remote tool call is sent.

Nine of 27 built-in Agent bindings are now migrated. The other 18 bindings and the
nested-workflow, CPU-cooperation, timeout, emergency-abort, terminal-race, E2E and
performance/capacity gates remain open. Native HTTP still uses its existing
blocking backend; the cancellation proofs execute built WASM components.

```sh
scripts/build-agent-components.sh
cargo test -p runtara-agent-mailgun -p runtara-agent-teams -p runtara-agent-mcp
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
```

Results: the normal build regenerated all 27 Agent components and both shared
workflow components with metadata. The focused native suites passed 35 tests
(18 Teams tests and 17 MCP tests; Mailgun has no existing native tests). All 37
component cancellation tests passed, including the eight new messaging/MCP test
functions. The full emitted-workflow suite passed 274 tests with four test threads,
including all 24 cooperative lifecycle cases; its two manual release benchmarks
remain ignored. Affected-crate all-target Clippy with the existing integration
test features, formatting and diff whitespace checks passed. No real service,
database/server E2E, Linux qualification or new performance/capacity measurement
was run in this stage.

## S3, Azure Blob Storage and SFTP I/O

The existing S3 and Azure Blob Storage components now use callback exports and
await their normal proxy requests across all ten capabilities each. The shared
`runtara_http::presign` helper is now async; both in-repository callers await it.
Signing remains in the existing proxy service with unchanged request fields,
tenant headers and endpoint derivation. The SFTP component's four capabilities
await their existing native-service HTTP endpoint. There is no new feature flag,
alternate build path, host invocation registry or workflow scheduler.

Seven built-component test functions cover 36 blocked-request cases: uploads,
copies, deletes, both stages of downloads, presigning through both supported
proxy URL shapes, and all four SFTP capabilities. Every blocked case is exercised
before response headers and during a partial response body. Standard cancellation
must close local I/O, allow an independent sibling to finish and permit a fresh
call in the same Agent instance. A cancelled download HEAD must not fall through
to GET, and a cancelled presign must not become an ordinary soft failure.

The same tests preserve non-cancellation behavior: provider-specific deletion
status handling (including already-absent objects), successful GET after an
ordinary HEAD failure, presign output and soft failures, and SFTP HTTP, envelope
and output error classification. Fixture assertions cover encoded object paths,
copy-source headers, upload bytes, Azure blob type, presign parameters and SFTP
forwarding. All endpoints are local fixtures; no storage account or SSH server
is contacted.

Three new emitted-DSL test functions exercise seven workflows: both providers'
download HEAD and GET waits, both providers' presign waits, and the SFTP service
wait. The existing root lifecycle command requires cleanup before acknowledgement
and prevents retries, `onError` recovery and normal workflow completion.

These guarantees stop the guest's pending work. A storage write already accepted
by a service may still finish; closing the SFTP wrapper's HTTP request does not
prove termination of native SSH work. Neither cancellation nor its tests add
rollback or transaction ownership. Native HTTP retains its blocking transport.

Twelve of 27 built-in Agent bindings are migrated. The other 15 bindings and the
nested-workflow, CPU-cooperation, timeout, emergency-abort, terminal-race, E2E and
performance/capacity gates remain open.

```sh
scripts/build-agent-components.sh
cargo test -p runtara-http --features native -p runtara-agent-s3-storage -p runtara-agent-azure-blob-storage -p runtara-agent-sftp
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
```

The normal build regenerated all 27 Agent components and both shared workflow
components with metadata. All 17 focused native tests and 44 component cancellation
tests passed. The full emitted-workflow suite passed 277 tests with four test
threads, including all 27 cooperative lifecycle tests; the two manual release
benchmarks remain ignored. Affected-crate all-target Clippy with the existing
integration test features, formatting and diff whitespace checks passed. No
database/server E2E, Linux qualification or fresh performance/capacity measurement
was run in this stage.

## SQS long polling and queue operations

The existing SQS component now uses its normal callback export and awaits the
shared SQS HTTP helper across all 17 capabilities. It retains the AWS JSON
protocol, opaque connection reference, proxy-owned signing and existing 65-second
request timeout. No product flag, alternate component, host task registry or
automatic queue cleanup was added.

Five built-component test functions cover every capability with pending headers
and a partial response body (34 cancellation cases). Each standard cancellation
must release the local request, let the independent sibling finish and permit a
fresh ListQueues call in the same Agent instance. The fixture rejects any retry,
implicit DeleteMessage or visibility adjustment between cancellation and that
fresh invocation. Request checks cover long-poll and visibility parameters, FIFO
fields, message attributes, batch entries, queue configuration and tags, AWS
target/service headers, connection and tenant identity, and the existing timeout.

Additional built-component cases exercise every capability's successful response
and HTTP 403/503 soft-error behavior. They preserve receive receipt handles and
message attributes, batch successes alongside per-entry failures, queue metadata
and pagination. Malformed proxy transport remains a retryable network error;
missing connection configuration remains a permanent error before I/O.

Three normal emitted workflows exercise a receive-then-delete graph. Cancellation
while receive headers or its body are pending prevents deletion. A third case
completes receive, maps the returned receipt into DeleteMessage, then cancels that
pending delete. Each case requires cleanup before lifecycle acknowledgement and
bypasses retries, `onError` and workflow completion. These tests do not restore a
remote message's visibility or undo an accepted send/delete. They use local HTTP
fixtures only, without an AWS account or real queue.

Thirteen of 27 built-in Agent bindings are migrated. Five network Agents and nine
CPU-oriented Agents still need qualification, along with nested workflows,
timeouts, terminal races, emergency abort, artifact cleanup, E2E and performance
gates. The native HTTP backend remains blocking.

```sh
scripts/build-agent-components.sh
cargo test -p runtara-agent-sqs
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
```

The normal build regenerated all 27 Agent components and both shared workflow
components with metadata. All 11 SQS native tests, 49 component cancellation tests
and 30 focused emitted lifecycle tests passed. The full emitted-workflow suite
passed 280 tests with four test threads; its two manual release benchmarks remain
ignored. Affected-crate all-target Clippy with the existing integration test
features, formatting and diff whitespace checks passed. No real AWS service,
database/server E2E, Linux qualification or new performance/capacity measurement
was run in this stage.

## Upstream integration and artifact isolation

The feature branch incorporates upstream `main` at
`4eff9cdf83342ad45ef89f73181934c5485085a0` (workspace version 8.9.10),
including the environment worker/repository refactors. The merge required no
conflict resolution. Public Stop and terminal-race behavior are still the P3
limitations described at the top of this record.

The first component run after integration loaded stale synchronous artifacts
from the shared checkout's target directory: 23 tests passed and 26 cancellation
tests failed. Inspection of the HTTP artifact showed a synchronous lift even
though this worktree's source declared callback bindings. A fresh normal bundle
build in a directory dedicated to this worktree restored the callback ABI; all
49 component cancellation tests then passed. The fixture now rejects an artifact
without a callback lift before starting cancellation waits. This preflight was
also run against the stale artifact and failed immediately as intended; runtime
cancellation, cleanup and instance reuse remain the actual proof.

Use separate output paths for this worktree and each future benchmark baseline:

```sh
RUSTC_WRAPPER= SQLX_OFFLINE=true CARGO_BUILD_JOBS=4 \
CARGO_TARGET_DIR=/Users/volodymyrrudyi/work/runtara/target/wasm-emitter-audit-components \
scripts/build-agent-components.sh

export RUNTARA_AGENT_COMPONENTS_DIR=/Users/volodymyrrudyi/work/runtara/target/wasm-emitter-audit-components/wasm32-wasip2/release
export CARGO_TARGET_DIR=/Users/volodymyrrudyi/work/runtara/target/selective-isolation-check
```

The isolated normal build produced all 27 Agents and both shared workflow
components with metadata. The environment/server unit suites passed 143 and
1,178 tests respectively. The full emitted-workflow suite passed 280 tests with
four test threads; the two manual benchmarks remained ignored. All-target Clippy
passed with the existing environment,
server, component-host and workflows integration-test features enabled. No
live-database/server E2E, fresh performance measurements or Linux qualification
was performed during this integration check.

## Stripe request cancellation

The existing Stripe component now uses its standard callback export and awaits
GET, form-encoded POST and DELETE through the shared HTTP client. All 26
capabilities use that path. The proxy still owns authorization and credential
injection; the guest retains the same connection reference, relative paths,
30-second request timeout, request encoding and provider error mapping. Generated
capability metadata is byte-for-byte identical before and after the change.

The built-component fixture covers all 26 capabilities plus both immediate and
period-end subscription cancellation: 27 request cases, each interrupted while
headers or a response body are pending. These 54 cancellation cases require I/O
closure, an independent sibling's completion and a fresh balance request in the
same Agent instance. The next request must be that explicit fresh invocation;
no automatic retry, refund, subscription change or compensating API call is
allowed as cancellation cleanup. Stripe's `cancel-subscription` capability is a
provider operation, distinct from cancelling the running component call.

Successful cases preserve returned objects, list pagination, metadata, nested
form fields and both subscription-cancellation modes. Separate checks cover
GET/POST/DELETE errors at 400, 401, 403, 429 and 503, retry-after seconds and
millisecond precedence, malformed proxy transport, invalid provider JSON and
missing connection configuration before I/O. The fixture checks proxy routing,
tenant and connection identity, absence of an authorization header in the guest
request, timeout, query parameters and percent-encoded form fields. Fixtures use
local synthetic responses only; no Stripe account, payment or invoice is touched.

Three normal emitted workflows exercise create-invoice followed by finalize.
They cancel during creation headers, during its response body, or during
finalization after a successful create and output-reference mapping. Cleanup
must precede lifecycle acknowledgement; retry, `onError`, downstream work and
normal completion must not occur after root cancellation. These assertions do
not imply that a remote provider reverses an already accepted request.

Fourteen of 27 Agent bindings are migrated. QuickBooks, HubSpot, SharePoint and
Shopify, the nine CPU-oriented Agents, nested workflows, timeout/grace and
terminal races, superseded API cleanup, E2E and performance/capacity gates remain
open. Native HTTP execution remains blocking.

```sh
scripts/build-agent-components.sh
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
```

Use the isolated build/output directories from the preceding section. The normal
bundle build regenerated all 27 Agents and both shared workflow components with
metadata. All 54 component cancellation tests passed, including the five Stripe
test functions. The full emitted-workflow suite passed 283 tests with four test
threads, including all 33 cooperative lifecycle cases; two manual benchmarks
remained ignored. Affected-crate all-target Clippy with the existing integration
test features, formatting and diff whitespace checks passed. No live-provider
or database/server E2E, Linux qualification or fresh performance/capacity
measurements were run for this stage.

## QuickBooks read and write cancellation

All seven QuickBooks capabilities now await their existing GET/POST helper under
the standard callback export. The normal build emits the updated component;
there is no new product flag, alternate runner or host task registry. Proxy-owned
credentials and company/realm routing, opaque connection references, minor-version
selection, the 30-second HTTP timeout and response/error mapping are unchanged.
The generated capability metadata is byte-for-byte identical to the prior build.

Five built-component test functions cover all capabilities and both sparse/full
update forms. Eight request cases are cancelled during pending headers and a
partial body, for 16 cancellation cases. They require local I/O closure, sibling
completion and a successful fresh read in the same Agent instance. The fixture
rejects an automatic retry, token refresh, delete or compensating request between
cancellation and that explicit fresh read.

Successful response checks preserve entity IDs and SyncTokens, query rows and
pagination envelopes, report objects, deleted status and CDC changed/deleted
records. Wire checks cover percent-encoded query/ID/timestamps, sorted report
parameters, API versions, sparse versus full bodies and authoritative Id/SyncToken
injection. HTTP 400/401/403/429/503 retain the existing retry classification for
GET and POST. Malformed transport, invalid provider JSON, missing connections
and empty CDC entity lists retain distinct errors; empty successful bodies retain
their existing output shape. All fixtures use local synthetic data, with no real
Intuit account or accounting records.

Three emitted read-then-update workflows cancel during read headers, during its
body, or during update after a successful read. The last case checks that output
references supply the returned Id and SyncToken to the update. Cancellation must
close pending I/O before lifecycle acknowledgement, and must bypass retries,
`onError`, downstream work and normal completion. No local assertion promises
that a provider undoes an accepted write.

Fifteen of 27 Agent bindings are migrated. HubSpot, SharePoint, Shopify and the
nine CPU-oriented Agents remain, together with nested workflow cooperation,
public Stop/grace, timeout and terminal-race semantics, superseded API cleanup,
E2E and performance/capacity qualification.

```sh
scripts/build-agent-components.sh
cargo test -p runtara-agent-quickbooks
cargo test -p runtara-component-host --features component-integration-tests --test cooperative_cancellation -- --test-threads=4
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=4
```

Use the isolated output directories recorded above. The normal bundle build
regenerated all 27 Agents and both shared workflow components with metadata.
All 12 native QuickBooks tests and 59 component cancellation tests passed. The
full emitted-workflow suite passed 286 tests with four test threads, including
all 36 cooperative lifecycle test functions; two manual benchmarks remained
ignored. Affected-crate all-target Clippy with the existing integration-test features,
formatting and diff whitespace checks passed. No live-provider or database/server
E2E, Linux qualification or fresh performance/capacity measurement was run.

## Fresh paired measurement harness

The historical baseline/isolation reports remain unchanged. The new shared
`tests/cooperative_measurement/mod.rs` module measures the normal compiler and
runtime at each source revision. Its corpus covers the original 11 workloads
plus Finish payloads of 16,383, 16,384 and 16,385 bytes. Manual release runs use
five warmups and 1,000 prepared samples per workload, with three separately
counted cold builds. Raw samples are retained; cold runs do not publish tail
percentiles from three observations.

The harness records graph/input/dependency hashes, complete `.wasm` and gzip
sizes, workflow logic and serialized native sizes, canonical ABI counts and
component imports. It rejects the superseded custom task-service import.
Emission, composition, native compilation, prepared linking, first completed
execution, prepared full runs and durable replay are measured separately.
Successful runs validate output ranges/counts and exact replay identity. The
separate random-double Agent measurement excludes instantiation, export lookup
and host teardown; it does not stand in for a parent-step phase measurement.

`scripts/measure-cooperative-workflows.py` copies this identical test module into
an upstream worktree and adds its module registration. It does not patch baseline
production sources or Cargo configuration, and rejects unrelated baseline edits
and a dirty candidate. The manifest records both revisions, lockfiles, test-host
differences, compiler versions, executable hashes, build paths and host load.
Normal component/release builds complete before three fresh-process measurement
pairs run in baseline/candidate, candidate/baseline, baseline/candidate order.
Per-run JSON, outcomes, logs and generated `.wasm`/graph/input artifacts are kept.
An error stops the run and records failure instead of publishing a successful
comparison; source changes after building also fail the run.

The 14-workload smoke test passed, including artifact export and standalone Agent
invocation. Existing-feature all-target Clippy, formatting and whitespace checks
passed; the Python driver passed syntax and CLI checks. Release numbers are not
yet available at this commit. The separately built upstream component bundle at
`4eff9cdf83342ad45ef89f73181934c5485085a0` is ready in its own target directory.

```sh
python3 scripts/measure-cooperative-workflows.py \
  --baseline /Users/volodymyrrudyi/work/runtara/.claude/worktrees/cooperative-baseline-4eff9cdf \
  --candidate /Users/volodymyrrudyi/work/runtara/.claude/worktrees/wasm-emitter-audit \
  --baseline-target /Users/volodymyrrudyi/work/runtara/target/cooperative-baseline-4eff9cdf-components \
  --candidate-target /Users/volodymyrrudyi/work/runtara/target/wasm-emitter-audit-components \
  --output /private/tmp/cooperative-measurements-20260907
```

This first comparison will be interim: only 15 of 27 Agent bindings are migrated.
Parent-step instrumentation/cost, full DSL validation timing, real HTTP waits,
public-server terminal timing, cancellation/abort latency, Linux capacity and
resource-soak measurements remain required before final qualification.

## First fresh no-cancellation comparison

[The interim report](research/workflow-cooperative-cancellation-comparison.md)
and its [raw samples/manifest](research/workflow-cooperative-cancellation-comparison.json)
compare upstream `4eff9cdf83342ad45ef89f73181934c5485085a0` with candidate
`103c32cee4fccc1f52ac960c21a86294e8c48d77`. Both used independently built release
artifacts and the identical shared harness. All six processes completed the
14-workload corpus with five warmups, 1,000 prepared samples per condition,
three cold samples, replay validation and a separate random-double Agent timer.
Preserved `.wasm`, graph and input artifacts were checked against their hashes.
Sources and relevant component/metadata inputs remained fixed throughout each
run. No custom task-service import participates in the measured artifacts.

The first driver attempt completed its baseline process but failed to parse a
libtest-prefixed report. A parser fix and two regression tests were committed;
all three pairs were then restarted in a fresh output directory. The failed
attempt remains separate and none of its samples are used in the report. The
completed cohort's logs and preserved artifacts remain under
`/private/tmp/cooperative-measurements-20260907-r2`.

The single non-durable random workflow's `.wasm` grows by 0.15%; the 100-step
chain grows by 8.0% raw, 0.50% gzip and 8.5% serialized native bytes. The chain's
native compilation is materially slower in all three pairs, and its first-result
medians are 2.26–3.64 times baseline. Generated chain logic approximately doubles
in size while the utils Agent and JSON stdlib binaries are identical. Repeated
inline wait/poll/cleanup emission is a concrete optimization target, subject to
preserving caller-local state and the cancellation/cleanup/acknowledgement tests.
Prepared runtime results and their per-process variation are in the report; do
not attribute all timing differences to a single code change.

This closes the first fresh paired measurement task, not the full performance
gate. Remaining Agent/nested-workflow integration, parent-step instrumentation,
full validation timing, real HTTP and public-server timing, cancellation/abort
latency, signal-poll/DB cost, Linux capacity/soak and deployment budgets remain
open. Re-qualify after the remaining implementation and superseded-path cleanup.

## Shared guest wait functions

The emitter now places polling, retained-signal handling, checkpoint-signal
handling, sequential waits and parallel-window waits in shared core-WASM
functions inside the workflow artifact. Fifteen explicit i32 parameters/results
carry invocation-local handles, window boundaries and retained signal identity;
there is no heap frame or global cancellation state. Scratch cursors remain
local to each function. An extra result tells the entry to continue, propagate
an error or return suspended using its existing ABI. Cleanup still resolves all
owned subtasks before acknowledgement or terminal error reporting. Eagerly
returned Agent calls skip the wait helper entirely.

This updates the existing emitter path. Runtime-omitted workflows emit only
runtime-free wait helpers; workflows without Agents retain their existing
lowering. A cache identity revision replaces previously emitted inline waits
when workflows are recompiled. No new component imports or product flags are
introduced.

A structural regression test validates 1- and 100-Agent core modules across CLI,
lifecycle and capability entry ABIs (including the runtime-omitted capability
shape), bounding additional code to less than 3,300 bytes per site. Existing
checkpoint-order tests now follow calls into defined helpers while distinguishing
helper returns from workflow returns. Cancellation integration coverage continues
to check no launch after pre-call Cancel, pending headers/body cleanup, every
parallel scheduling form, poll failures, cleanup-before-acknowledgement, and
retained Pause/Shutdown receipts. Fresh paired release measurements are required
before declaring the size/compilation regression resolved; the first report
remains an immutable pre-optimization snapshot.


### Shared-helper verification and measured follow-up

Commit `e8785f37637799bcee04a7e6face54af4fc200f4` implements the factoring.
The structural suite passes 134 tests across the existing compiler cases and
new growth guard. The full direct execution suite passed 287 tests with three
manual benchmarks ignored before the final eager-return fast path; after that
adjustment, all 36 cancellation tests plus the 14-workload measurement smoke
passed (37 test functions, one manual benchmark ignored), and the 134 structural
tests passed again. The component integration feature enabled all 59 component
cancellation tests, which passed. Integration-feature Clippy, workspace formatting
and the workspace Clippy commit hook passed. Components were built through the
normal script for each separately targeted release revision before measuring.
No database/server E2E or Linux capacity tests were run for this optimization.

The [follow-up report](research/workflow-cooperative-shared-waits-comparison.md)
and [raw samples/manifest](research/workflow-cooperative-shared-waits-comparison.json)
record three complete new pairs against the same upstream `4eff9cdf` baseline.
The driver source/fixture hashes are retained, each run completed successfully,
and all preserved artifact/graph/input hashes were independently checked. Logs,
outcomes and artifacts remain in `/private/tmp/cooperative-shared-waits-20260907`.
The earlier cooperative report remains unchanged.

The 100-Agent artifact drops 220,217 raw bytes versus the inline cooperative
revision, leaving 1.75% raw overhead over upstream. The single-Agent artifact
grows 4,019 bytes versus that revision, reaching 0.27% overhead over upstream.
The measured cold ratio for the long chain remains 1.49–2.15× upstream and its
prepared median changes range from −0.3% to +9.6%. These timing observations do
not establish helper overhead or a causal speedup: the recorded host load
varied from 29.05 to 116.42 on 16 logical CPUs. A controlled timing run, further
small-graph size work and remaining qualification metrics are still required.


## HubSpot CRM cancellation

All 43 HubSpot capabilities now use standard callback bindings and await the
existing five HTTP helpers (GET, POST, PATCH, PUT and DELETE). Cancelling a
pending request drops its guest future and resolves its standard I/O subtask.
The existing proxy policy, opaque connection ID, URLs, bodies, timeouts and
metadata remain in place. No native workflow orchestration or custom task API is
added. Native metadata/test builds retain the blocking transport used by the
other migrated Agents; they do not prove native I/O cancellation.

The built-component fixtures cover all published capability IDs, including
business units, object properties, contacts, companies, deals, quotes, line
items, owners, pipelines, associations and webhook subscriptions. A metadata
coverage assertion fails on a missing or duplicate fixture capability. Every
capability is cancelled while awaiting headers and while reading a partial
response body: 86 blocked-I/O cases. Each requires connection closure before the
sibling resumes and then invokes the same component instance successfully.

Normal-response cases cover all 43 capabilities, query escaping without assuming
HashMap order, CRM/search payloads, pagination, association PUT and webhook
updates, plus empty-body success for associations and deletion. Error cases
cover all five HTTP methods, provider status/retry classification, millisecond
retry-hint precedence, transport-envelope decoding, malformed JSON, missing
connections and unknown capabilities. All traffic uses local synthetic proxy
fixtures; no HubSpot account or customer data is involved.

Two existing error-handling edge cases were fixed and tested in native and WASM
execution. An overflowing `Retry-After` seconds value is ignored using checked
conversion; rate limiting remains retryable with ordinary workflow retry policy.
Truncating a provider's error body now respects UTF-8 character boundaries,
avoiding a panic on multibyte text at the 512-byte cutoff. These changes preserve
ordinary error envelopes and keep such failures distinguishable from cancellation.

Three emitted DSL tests execute a contact read followed by a contact update.
They cancel pending read headers, a partial read body, and the update after a
successful read. Each checks cleanup before the lifecycle receipt and prevents
retries, `onError` recovery, following writes and normal terminal completion.

The normal build script rebuilt all 27 Agent and two shared workflow components
in the worktree-specific target directory. HubSpot's metadata is byte-identical
to its pre-migration version. The two native boundary tests, six focused
component test functions and three emitted HubSpot workflow tests passed;
integration-feature Clippy passed for HubSpot, component host and workflows.
All 65 component cancellation tests and the full direct workflow suite
(290 passed, three manual benchmarks ignored) passed. No database/server E2E
or controlled performance/capacity runs were performed for this Agent stage.

The remaining Agent bindings are SharePoint, Shopify, compression, crypto, CSV,
datetime, text, transform, utils, XLSX and XML. CPU-bound operations still need
bounded cooperation points or an explicit emergency-abort limitation. Nested
workflow-agent ownership, cooperative deadlines/E128 retirement, public Stop and
independent grace escalation, terminal races, controlled performance/capacity
qualification and superseded-path cleanup remain open.


## Shared Agent component macro and remaining HTTP migrations (2026-09-07)

`runtara-agent-macro::agent_component!` now owns the callback binding generation,
async `invoke` export, dispatch and conversion to the WIT error record for **all
27 built-in Agents**, including earlier migrations. Agents list their annotated
Rust capability functions; `#[capability]` emits the wire-ID constants and uniform
async invocation adapters. Generated dispatch uses constant string match arms,
without a boxed registry or another runtime layer. Native executor descriptor
types are unchanged. The old per-Agent bindings, handwritten dispatchers and
error-conversion functions have been removed.

There is no cancellation annotation, product flag, custom task manager or host
workflow routing. The standard component binding owns cancellation delivery;
`runtara-http` provides the shared awaitable transport. Agent helpers explicitly
await it. The macro adapts synchronous capabilities too, but cannot insert safe
cooperation points into arbitrary blocking code. Compression, crypto, CSV,
datetime, text, transform, utils, XLSX and XML still need bounded CPU/blocking
cooperation or explicit emergency-abort qualification. A callback lift alone is
not proof that an operation can be interrupted while computing.

The shared converter retains explicit retryability, attributes and retry delay,
and otherwise infers retryability from the transient category. This removes the
old CPU-shim inconsistency that defaulted missing retryability to false; their
current permanent errors remain non-retryable. Datetime retains its special
empty/whitespace-to-null input decoder. A built-component test confirms the
existing limitation: `get-current-date` rejects that null during typed validation;
`{}` applies defaults. The refactor does not silently repair this unrelated input
behavior.

SharePoint's 14 capabilities and Shopify's 50 capabilities now await the shared
HTTP API throughout their helper chains. Verification concentrates on distinct
I/O sites and control flow: all nine SharePoint request sites, metadata fallback,
page-token preservation, simple uploads, copy initiation/monitoring, upload
session creation and both sides of the existing 4 MiB upload boundary. Shopify
cases cover GraphQL error envelopes, read/delete/replace mutations and bulk
updates that continue after an ordinary item failure. Cancellation during any
of those awaited calls drops the invocation, so ordinary fallback/error-catching
logic cannot launch another request from it. Successful earlier mutations remain
external effects; cancellation does not roll them back.

The common WAT parent fixture now lays out larger inputs without overlapping its
second input or result heap. Limits remain explicit, and only the large-upload
proxy reader accepts the larger request envelope. Tests cancel both pending
headers and partial response bodies, require socket closure before the sibling
finishes, and invoke the same Agent instance again. The large-upload fixture
checks actual multi-chunk requests. It does not certify SharePoint's provider
chunk-alignment rule: the pre-existing 4 MiB constant is not a multiple of the
320 KiB unit described by its own source comment. Chunk HTTP-error handling and
copy-monitor HTTP-status interpretation also remain provider-specific follow-up
issues, outside this cancellation/plumbing change.

Six emitted DSL scenarios cancel SharePoint metadata/content requests and
Shopify media-read/deletion requests, including a completed first request. They
use normal composition, lifecycle signals and production I/O, and assert cleanup
before acknowledgement with no retry, recovery or terminal-success publication.
The shared-macro fixture additionally composes all 27 actual built components,
checks their callback lifts and error contracts, and exercises synchronous
random-double and datetime capabilities through the same export.

Validation completed with the pinned toolchain and isolated component/native
build directories:

- Normal `scripts/build-agent-components.sh`: all 27 Agents plus two shared
  workflow components. All 27 metadata artifacts, describing 305 capabilities,
  are byte-identical to the pre-consolidation snapshots; SharePoint/Shopify also
  match their pre-async metadata snapshots.
- `cargo test -p 'runtara-agent-*'`: 474 tests/doctests passed on the final source.
- Component cancellation suite: all 75 tests passed on the final built artifacts.
  The new provider cases include 50 pending-header/partial-body cancellation
  points, normal multi-request success and ordinary-error behavior.
- Full direct workflow suite: 296 passed, three manual benchmarks ignored.
  After the final constant-pattern dispatch adjustment and rebuild, all 45
  emitted cooperative-workflow tests were rerun and passed.
- Feature-gated Clippy for all Agent packages, component host and workflows,
  `cargo fmt --all -- --check`, and `git diff --check` passed.

No database/server E2E, controlled performance or capacity qualification was run
for this refactor. This stage does not close the public Stop/grace, nested
workflow, CPU cooperation, deadline/E128, terminal-race, compatibility, controlled
performance, capacity or superseded-path-removal gates in the governing plan.


## Terminal cancellation races and aborted outcomes (2026-09-07)

Before wiring the public Stop grace period, the shared lifecycle policy now
rejects a new cancellation receipt once a terminal outcome has been accepted.
The receipt identity and idempotency checks still run first: retrying an already
accepted receipt returns its existing disposition without applying the transition
again. A queued cancellation request alone does not undo completion. The existing
atomic backend guards decide between a completion write and a cancellation
acknowledgement; both cannot win.

The runner's post-exit fallback no longer calls the guest acknowledgement handler.
If a cancellation remains pending after the guest exits and the instance is still
running, it records status `cancelled` with termination reason `aborted`. It leaves
the command unacknowledged, so this cannot be mistaken for completed guest cleanup.
A completed, failed, cancelled or parked outcome is preserved, including its
output, error, completion timestamp and termination details.

The older interrupted-Delay escalation path likewise sets the existing whole-run
abort flag and advances the guest's epoch deadline without manufacturing a guest
receipt. Once abort is selected, runtime-host completion/failure/suspension events
are suppressed while the interrupt takes effect; normal terminal publication
waits for the post-exit path. Ordinary cooperative signal handling remains in the
guest, using the existing signal receipt. No workflow graph routing, child tasks,
new cancellation transport or custom task registry is introduced.

Forward migration `026_aborted_termination.sql` adds the `aborted` termination
reason. The server's typed runtime parser and JSON representation recognize it.
This changes no HTTP OpenAPI schema: the modified runtime type is not an OpenAPI
response schema, and the existing generated HTTP client has no termination-reason
field. The public Stop handler still needs to send the lifecycle cancellation,
honor grace independently of guest cooperation, and stop writing an unconditional
terminal cancellation. This change does not qualify that endpoint yet.

Tests cover completed/failed/cancelled outcomes, requests arriving before and
after completion, preserved result fields, repeated receipts, deterministic
cancellation-first ordering and sixteen concurrent completion/acknowledgement
races on both persistence backends. Database-backed runtime tests cover legacy
escalation, suppression of terminal events after abort selection, accepted
completion before abort, and post-exit classification without a receipt. An
embedded-runner test stops a spinning WASM instance and checks the final `aborted`
reason and still-unacknowledged command after the runner exits.

Verification: all 79 Core tests passed, including the in-memory conformance
sequence. The PostgreSQL conformance entry passed against a dedicated local test
container, exercising the same sequence and lifecycle matrix. All 234
database-backed Environment unit tests and five embedded-runner integration tests
passed. The five server termination-reason tests passed, including the new JSON
roundtrip. Feature-gated Clippy for Core, PostgreSQL, Environment and Server,
workspace formatting and diff checks passed. The forward migration was applied
to the isolated databases; no existing migration was edited.

No public HTTP Stop E2E or performance/capacity run is claimed. Public Stop/grace,
CPU cooperation, nested workflow cancellation, deadlines/E128 and the remaining
plan gates stay open.


## Stop signals, cleanup grace and whole-run abort (2026-09-07)

The environment Stop handler now delivers the existing lifecycle `Cancel` and
arms an absolute monotonic deadline for the currently owned execution. Acceptance
means cancellation was requested; it does not immediately publish `cancelled` or
release the persisted runner handle. Guest acknowledgement owns cooperative
terminal publication. For an unacknowledged cancellation, actual runner exit owns
the existing `cancelled`/`aborted` fallback, and the monitor releases registry and
launch ownership after exit. Already accepted terminal outcomes are preserved.

The server's `ExecutionEngine::stop` previously called a signal-only
`RuntimeClient::cancel_instance`, bypassing `handle_stop_instance`. That method now
reuses `stop_instance`, so both public Stop and the server's cancellation caller
use the existing five-second default grace. Low-level signal delivery remains a
signal operation; it does not acquire a new orchestration responsibility.

Grace uses Tokio's timer and watch channel on the existing whole-run task record.
One timer task per detached execution waits independently of guest future polling;
it sets the runner's existing abort flag, consumed by the existing Wasmtime epoch
and host-wait watchdogs. There is no per-Agent task table, graph interpretation,
child Store, or new guest import. Completion owns timer disposal through the
existing RAII guard; the timer holds a weak reference to its exact execution and
cannot cancel a replacement. Its memory/scheduling cost remains to be measured
in the controlled capacity run. This stage changes no emitted WASM or component
binary.

The deadline starts when the Stop handler receives the request, including time
spent delivering the signal. Repeated requests may shorten it but cannot extend
it. Zero arms immediate full abort; a deadline already elapsed during delivery
fires immediately when armed. An unrepresentable duration fails before any signal
or state mutation. Other execution/resource limits can still terminate a run
sooner. This is whole-execution grace, not a cooperative step timeout; E128 stays
in place.

Queued/unconfirmed launches retain their existing atomic pre-start cancellation.
Parked executions retain the existing signal/scheduler cancellation path. A run
that completes or parks during handle lookup/arming is rechecked; terminal
completion/failure is not overwritten. If Core still says active but the local
runner cannot own/arm the handle, Stop reports failure **after** persisting the
signal. It does not claim a remote grace deadline was enforced. Routing a Stop to
another live server's owner and distributed grace durability remain unqualified.
A host restart loses in-memory timers along with that host's guest executions;
existing durable recovery still owns their persisted records.

Verification added:

| Cases | Evidence |
|---|---|
| No request; 60-second grace; later request; shortened, zero and elapsed deadlines | Paused-time runner tests check the exact firing instant and that grace never extends |
| Early completion, replacement generation, disappeared owner | Runner tests prove timer disposal, weak ownership, capacity return and no stale abort |
| Request acceptance, zero grace, overflow, missing/stale handle | Handler tests use real PostgreSQL and a controllable runner; acceptance does not fabricate acknowledgement, terminal state or registry cleanup |
| Queued/parked, completed/failed/cancelled, completion/parking during arming | Handler tests cover pre-start cancellation, preserved terminal outcomes, and deterministic retirement between lookup and arming |
| Infinite invocation and infinite initializer | Real embedded WASM through the Stop handler exits after grace, retains an unacknowledged command and `aborted` outcome, returns capacity, and permits a fresh run |
| Normally composed HTTP with pending headers or a partial body | DSL -> composed artifact -> embedded runner -> Stop handler -> real persistence; socket closes, guest acknowledges, retry/onError/Finish do not run, and the monitor releases the handle before the ten-second grace is needed |

The composed tests use the existing artifact-dependent integration-test feature
and CI job (currently named `scoped-workflow-integration-tests`). They explicitly
assert there is no isolation manifest or scoped Agent in their compiled artifact;
the test gate selects prerequisites, not production behavior. The partial-body
test supplies an unfinished response but does not independently instrument the
host's exact body-read phase. Cleanup-before-ack ordering at that phase continues
to rely on the existing component/emitter proofs; the new test joins those paths
to production persistence and Stop.

Validation commands use the pinned toolchain, isolated native/component targets,
and an owned ephemeral PostgreSQL fixture:

```sh
cargo test -p runtara-environment --features db-integration-tests --lib
cargo test -p runtara-environment --features db-integration-tests --test embedded_runner_test --test handlers_test
cargo test -p runtara-environment --features scoped-workflow-integration-tests --test cooperative_stop_test --test scoped_runner_test
cargo clippy -p runtara-environment -p runtara-server --features runtara-environment/scoped-workflow-integration-tests,runtara-server/db-integration-tests --all-targets -- -D warnings
```

Results: all 238 environment library tests, 45 handler tests, eight embedded-runner
tests, two composed HTTP Stop tests and seven existing packaged-runner tests
passed. Feature-gated environment/server Clippy, workspace formatting and
`git diff --check` passed. No full HTTP-server process, credentials, external
provider endpoints, new WASM build, or performance acceptance run was used.

Full authenticated-server E2E, a deliberately stalled standard cancellation
acknowledgement, blocking native calls, nested workflow-agent ownership,
cooperative deadlines/E128, distributed owner routing, controlled performance and
capacity measurements, and removal of the superseded experiment remain open.


## Grace while standard cancellation is stalled (2026-09-07)

The `cooperative_stop_test` now includes an adversarial callback Agent fixture
that exposes the normal HTTP capability interface. It starts real host-mediated
HTTP I/O and waits. When the normally emitted parent requests standard
`subtask.cancel`, callback event 6 starts a separate `/cleanup-entered` request
and then waits forever without `task.cancel` or `task.return`. The test endpoint
observes that distinct request, proving execution reached the cancellation
callback; a timeout before cancellation delivery would fail this precondition.

The parent is now blocked waiting for standard cancellation resolution. Before
grace expires, the test requires Core to remain running, the lifecycle command to
remain pending, the registry handle to remain present, and the runner permit to
remain held. At the five-second grace deadline the existing independent timer
raises the whole-run abort flag. Teardown closes both the original request and
the stalled cleanup request, publishes `cancelled` with reason `aborted`, leaves
the command unacknowledged, and returns runner/monitor capacity. No recovery,
retry or Finish output is allowed. The active execution timeout is thirty seconds,
so it cannot stand in for the grace deadline.

Two cases cover original I/O waiting for headers and holding an incomplete body.
The ordinary built-HTTP-Agent cases run beside them and must still acknowledge
cooperative cleanup before their grace deadline. The adversarial WAT lives in
`tests/cooperative_stop/stalled-cleanup-agent.wat`; its binary is generated only
in a temporary fixture directory, with private copies of production shared
components and unchanged capability metadata. Production staged artifacts are
never modified. Both paths use normal static composition, the production Stop
handler, production HTTP I/O and real PostgreSQL; neither carries an isolation
manifest nor uses a custom task catalog.

All four composed Stop tests passed on the pinned stack, together with
artifact-feature environment Clippy, workspace formatting and `git diff --check`.
This establishes that
standard synchronous `subtask.cancel` may remain unresolved while the independent
host grace still ends the whole execution. No async-cancel extension, alternate
backend or production code change was necessary. It does not prove that an
arbitrary blocking native function returns or that remote server work is undone.
Full native-call/resource qualification, nested workflow-agent ownership,
cooperative step deadlines, authenticated-server E2E and controlled performance
remain open.

## Nested workflow-agent waits and parent cancellation (2026-09-07)

A workflow-agent can retain its ordinary generated call stack and still accept
standard cancellation. Its capability function is async-typed with a synchronous
lift, and its `waitable-set.wait` import is **cancellable**. Standard event 6
unwinds the guest's active sequential call and parallel window, resolves/drops
all nested handles and wait sets, then returns a typed `CANCELLED` error directly
from the capability. This bypasses the child's retry and onError control flow.
The composing caller receives the standard **RETURNED** resolution (2), which
its cancellation cleanup already accepts. It owns the cancellation decision;
the return is neither a root signal acknowledgement nor a completed workflow.

The dedicated Component Model fixture verifies nested cleanup, sibling survival
and reuse of the cancelled callee instance. It uses the unchanged production
Wasmtime 46.0.1 engine builder. It does not enable stackful async lifts, async
`subtask.cancel`, custom task imports or a host task registry. Built-in agents
continue using the shared callback-binding macro. A generated workflow-agent
uses its cancellable wait instead; both compose in the existing single artifact.

Non-durable Agent retries inside `AgentCapabilities` now await the existing
host-I/O timer through the same shared guest wait helper. Retry state and the
choice of another attempt remain in WASM. The compiler omits the root runtime
for statically qualified callable graphs, including ordinary Agent calls,
connections, control flow and parallel Split/branches. It still rejects Agent
closures requiring durability, logging/error runtime operations, debug
breakpoints, Wait/Delay, AiAgent, timeouts or unsupported Split/Embed backoff.
The complete static gate remains required; import analysis alone is not a
publication certificate. The server publishes with tracing disabled as before.
Top-level and retained legacy durable retry/suspend behavior is unchanged.

Coverage added:

- A standard synchronous-lift/cancellable-wait proof, including a subsequent call
  to the same callee and an unaffected sibling.
- Normally composed nested DSL calls: pending headers, partial body, two nested
  published levels, two parallel branches and two parallel Split items. Each
  child passes the production safety analysis and omits the root runtime;
  socket cleanup precedes the sole root acknowledgement. No retry, recovery or
  normal terminal publication follows cancellation.
- Long ordinary and recognized rate-limit backoff cancellation through two
  published levels. Normal retry runs preserve delays, attempt counts and nested
  success output; recognized rate limits outlive the ordinary retry count.
- Publication analysis and server preflight accept guest-local Agent waits while
  still rejecting durable runtime ownership. The emitter cache tag includes
  `parent-cancel=v1` so compiled artifacts are regenerated.
- Two compatibility cases pin existing retry gaps: `maxRetries: 0` bypasses even
  recognized rate-limit retries; `HTTP_429` uses ordinary retries rather than the
  separate rate-limit budget. See AUDIT-08 in the emitter audit. Neither semantic
  change is folded into cancellation support.

The retry cancellation fixture sends its signal after a complete error response
and a short handoff delay. It verifies bounded termination and no further retry,
not an instrumented timestamp for the native timer's entry/drop. Exact timer
resource accounting remains part of G10. Bounded CPU cooperation, blocking
native/resolver calls, durable workflow-agent suspension, Embed/While closure
qualification and targeted timeout behavior remain open. No new per-agent
binary, Store or host execution service is introduced. Retaining a guest stack
through a wait consumes instance resources until it returns or the whole run
aborts; this stage adds no durable parking contract for callable workflows.

Validation with Rust 1.97 and the isolated native/component directories:

- Normal build script: all 27 Agents and two shared workflow components built.
- Workflow library: 575 tests passed. Native emitter audit: 30 tests passed.
- Full direct-workflow execution suite: 305 passed, three manual benchmarks
  ignored. The final six nested retry cases also passed, including the two
  compatibility cases added after the full suite started.
- Full component cancellation suite: 76 passed. After clarifying the negative
  fixture's name, all three synchronous-binding/capability cases passed again.
- Server publication preflight: three tests passed.
- Feature-gated Clippy for workflows, component host and server, formatting,
  and `git diff --check` passed.

No database/full-server E2E was run for this stage. New size/timing and controlled
capacity comparisons remain pending; earlier measurement reports must not be
read as measuring this emitter revision.


## Loop cooperation and inline Embed cancellation (2026-09-07)

The pinned toolchain exposes the standard cancellable yield as
`thread.yield cancellable` / `[cancellable][thread-yield]`. It yields the current
guest execution to the Component Model scheduler and returns true when parent
cancellation is delivered. It does not create a native thread, another Store or
a task service. A focused composed fixture receives cancellation through this
yield (its readiness polling is non-cancellable), resolves a pending child,
allows its sibling to finish and reuses the cancelled instance. The production
engine configuration remains unchanged.

`cooperative_wait::emit_iteration_boundary` now provides a common cooperation
point for emitted While and sequential Split:

- Callable workflows yield to their composing parent, clean the same pending
  call/window state on cancellation and return directly from the invocation.
- Root workflows use the existing non-consuming lifecycle poll and shared
  cleanup/acknowledgement logic. A While only reaches legacy consuming checks
  when no active sibling window needs cleanup; pause/shutdown still defer to
  the existing safe boundary.
- Runtime-free While output no longer emits heartbeat/is-cancelled/check-signals
  calls to missing runtime indices. The previous feature analysis admitted this
  shape, but component validation failed before invocation. Its normal result
  now executes without a root runtime import.
- Legacy runtime-check errors keep their prior onError routing. The extra guard
  has an explicit branch-depth adjustment, with tests for both check sites.
- The artifact tag adds `loop-cooperation=v1`; previously compiled artifacts
  retain their original behavior until deliberately recompiled.

Before the fix, five of the six initial loop cases failed: runtime-free While
failed component validation, the root pure loops missed the supplied cancellation,
and a published CPU-only Split hit the execution watchdog. After the fix, root
loops observe cancellation without an Agent wait, and published loops accept
parent cancellation between iterations after a completed HTTP warmup. The latter
fixture uses a short handoff delay after the warmup response; it does not claim
an instrumented timestamp of the first CPU iteration. The separate canonical
fixture proves actual yield-based delivery and resolution.

Eight loop regression cases cover normal iteration counts/results, cancellation
and legacy error routing. Six additional emitted scenarios cover cancellation
inside a While HTTP body; a While beside a hanging HTTP branch; two inline Embed
scopes; partial HTTP body cleanup inside Embed; While inside Embed; and parallel
branches inside Embed. Root acknowledgement follows HTTP cleanup; nested retries,
recovery and terminal success do not run after cancellation selection. The
parallel-While host fixture rejects a consuming legacy check while the root
command is pending, so it cannot hide an early acknowledgement.

The cooperation bound is **one emitted iteration**, not a fixed wall-clock
latency. Expensive JSON/stdlib work, blocking native imports and arbitrary loops
inside the nine CPU-oriented built-in Agents still require separate qualification
or bounded yields. Sequential-loop polling/yielding adds per-iteration work; raw
WASM size, prepared execution time and signal/DB polling cost must be included in
the next paired measurements. Pure runtime-free workflows keep that mode. A
runtime-free root invoke has no lifecycle notification import; these changes do
not give that mode root signal delivery. Its whole-run abort remains available.
Two additional tests execute pure While/Split with `runtime: None` and check
complete iteration results.

This stage covers the tested inline Embed shapes; it does not authorize durable
workflow-agent suspension, every nested construct combination, per-step timeout
recovery or targeted cancellation. G1–G10 remain open where evidence is missing.
Validation with Rust 1.97 and the isolated native/component directories:

- Normal component build: all 27 Agents and two shared workflow components built.
- Workflow library: 575 passed. Native emitter audit: 30 passed.
- Full direct-workflow execution suite: 321 passed, three manual benchmarks
  ignored. The two hostless loop cases added after that run started passed
  separately, bringing the tested execution cases to 323.
- Full component cancellation suite: 77 passed.
- Feature-gated Clippy for workflows and component host passed.

No database/full-server E2E or new size/timing measurements were run for this
stage. The paired loop-cost and controlled capacity measurements remain open.


## Root Agent retry backoff (2026-09-07)

Non-durable Agent retries now use the same async-lowered host-I/O timer and
`cooperative_wait::emit_await_call` for every workflow ABI. Previously only
published workflow-agents selected that path; a root workflow synchronously
called `runtime.blocking-sleep`, which prevented guest lifecycle polling for
its entire backoff. The host still supplies an ordinary timer. The generated
WASM owns the wait handle, observes the root signal, resolves pending calls and
acknowledges cancellation through the existing cleanup boundary.

Two new root tests failed before this change: cancellation during ordinary and
recognized rate-limit backoff both reached the five-second execution watchdog
while waiting for a 60-second retry delay. Both now suspend cooperatively after
one HTTP request, acknowledge the command and bypass retry, recovery and success.
The fixture delivers cancellation after the error response is complete, with a
250 ms handoff delay; it does not instrument the exact timer-entry timestamp.

Six root cases reuse the same real HTTP/Slack fixture as the six published
workflow-agent cases. They cover cancellation in both backoff types, successful
ordinary retries, the independent rate-limit budget, zero ordinary retries and
HTTP_429 classification. No-cancel cases assert output, exact request counts and
a minimum elapsed delay. The compiler test also checks an async timer call after
delay calculation and the absence of a blocking-sleep call in the retry body.
The existing AUDIT-08 budget/classification discrepancies remain unchanged.

Durable lifecycle retries still checkpoint an absolute deadline and park, freeing
the Store; retained legacy durable paths keep their existing sleep semantics.
The artifact tag adds `retry-cooperation=v1`. No WIT interface, host task manager,
agent implementation or feature flag is added. Root waits reuse the existing
one-second lifecycle polling interval and retain their Store during non-durable
backoff. Responsiveness depends on scheduler and signal-service latency; this
is not a hard real-time bound.

Embed/Split retry helpers still contain blocking sleep paths. Non-durable Delay
also has a lower-level blocking emitter, but production support analysis rejects
it with `non-durable-delay` to avoid holding a runner. That rejection remains in
place. WaitForSignal's blocking polling loops are confined to legacy/capability
ABIs: the production root invoke parks on a signal after a miss, including AI
human-input waits. Migration must preserve that parking behavior, application-
signal consumption, safe pause/shutdown boundaries and error routing.
The timer import and shared-wait helpers are currently provisioned for graphs
with Agent calls; Agent-free wait graphs need explicit compiler provisioning
without adding host imports to pure runtime-free workflows.

Validation with Rust 1.97 and isolated native/component directories:

- Normal component build: 27 Agents and two shared workflow components built.
- Focused root/published retry suite: 12 passed (two new root cancellation
  failures reproduced before the change, then passed after it).
- Workflow library: 575 passed. Native emitter audit: 30 passed.
- Full direct-workflow execution suite: 329 passed, three manual benchmarks
  ignored. This includes durable Agent/Embed/Split retry parking and replay.
- Feature-gated all-target Clippy for workflows and component host, formatting,
  and `git diff --check` passed.

No database/full-server E2E or new performance/capacity measurements were run for
this stage. The component-host cancellation suite was last run for the preceding
loop stage (77 passed); this stage changes only workflow emission and its tests.


## Composite retry waits and timer-only workflows (2026-09-07)

Non-durable EmbedWorkflow and Split retries now call the same
`cooperative_wait::emit_timer_wait` helper as Agent retries. The helper lowers the
existing host-I/O timer and delegates to the shared standard call/wait/cleanup
path. Durable lifecycle retries still park; the remaining blocking retry arms
serve retained legacy durable export paths. No host task registry, agent-specific
wrapper, user annotation or backend selector is introduced.

Timer requirements are derived from the existing manifest, including nested
graphs and statically preloaded children, using the planner's existing retry
defaults. A graph with Agent calls already needs timers; an Agent-free graph now
also gets them when a non-durable Embed/Split retry can wait. A zero-retry graph
adds no timer import. The core provisions canonical subtask/waitable-set imports
and shared helper functions for timer-only graphs too. The component scaffolding
retains its `has_timers` requirement when an explicit runtime binding regenerates
`world.wit`; otherwise the executable could import timers while the returned and
on-disk WIT omitted them. The tests compare both metadata representations. The
artifact tag advances to `retry-cooperation=v2`.

The first eight Agent-free cases reproduced two failures with the old emitter:
root cancellation during Embed and Split backoff reached the five-second watchdog
while waiting for 60 seconds. All eight now pass: both cancellation paths, normal
retry exhaustion/recovery with exact attempt counts and elapsed delay, zero delay,
and zero retries. Four additional cases cover each retry type inside While and
inside a preloaded child whose outer Embed has retries disabled. They verify
that timer discovery reaches the inner graph and cancellation bypasses recovery
and terminal success. Error events establish child entry; cancellation follows a
250 ms handoff interval, not an instrumented timer-entry timestamp. No durable
sleep or checkpoint write occurs in these non-durable cases.

The real HTTP/Slack fixture now also exercises Split retries after an Agent
error. Its capability has zero retries, so the enclosing Split owns backoff.
Ordinary and rate-limit responses can be cancelled after one request; ordinary
no-cancel retries reach the successful third request. Existing root and published
Agent retry cases continue to exercise their own waits.

Real-Agent expansion exposed two **existing error-contract gaps**, recorded as
AUDIT-12. Agent failure lowering formats a text string containing JSON.
EmbedWorkflow expects its child error to be JSON and fails before it can retry;
Split treats the formatted text as an unclassified error and loses the separate
rate-limit budget. Both failures were reproduced after temporarily restoring the
original blocking composite sleep calls. That reference overlay was removed;
there is no selectable path in production. Four Embed tests retain the existing
HTTP/Slack parse failure (including fixtures scheduling cancellation after the
response), and one Split
test retains the existing ordinary-budget exhaustion. They are compatibility
checks, **not** evidence that Embed can cancel an Agent-error backoff it never
enters. Those fixtures do not establish post-terminal signal handling. Structured
Error-step failures do reach the tested Embed backoff.

This stage does not relax workflow-agent publication gates for Split/Embed
retries, change non-durable Delay rejection, or change root WaitForSignal parking.
Further published-composite qualification, structured error propagation,
per-step timeouts and performance/resource measurements remain open. The shared
root wait uses the existing one-second signal polling interval and holds a Store
until it returns. Timer-only workflows acquire that import/helper cost only when
required; fresh paired size/timing measurements must quantify it.

Validation with Rust 1.97 and isolated native/component directories:

- Normal component build: all 27 Agents and two shared workflow components built.
- Agent-free composite cases: 12 passed; real-Agent/root/published retry cases:
  20 passed, including the five explicit existing-error-contract checks.
- Workflow library: 575 passed. Feature-gated all-target Clippy for workflows
  and component host passed. Formatting and `git diff --check` passed.
- Full direct-workflow execution suite: 349 passed, three manual benchmarks
  ignored, including durable retry parking and replay.
- Strengthened hostless While/Split checks: two passed with no timer import.
  Native emitter audit: 30 passed.

An initial full-suite run and two follow-up compilations were interrupted by
filesystem exhaustion. Space subsequently became available externally; no caches
were removed by this task. The successful runs above are fresh reruns after that
interruption. No database/full-server E2E or fresh performance/capacity
measurements were run.

The interactive pattern lab now includes AUDIT-08 through AUDIT-12. It separates
historical failures, tested current behavior and unimplemented proposals; passing
compatibility tests are explicitly labelled as evidence of existing defects.
Node data checks verified all 98 scenarios, new audit anchors and linked Rust
test names. A headless Chromium pass exercised every route and next/reset
controls without script errors; desktop and 390-pixel mobile renders were
inspected, with no page-level horizontal overflow. These are illustrative traces,
not an in-browser WASM runtime or new performance measurements.

## Published Split retry cooperation (2026-09-07)

The workflow-agent publication check now permits non-durable Split retry waits
when the complete declared graph has no root runtime requirements. It uses the
same feature analysis and safety walk as callable Agent backoff. This deliberately
extends the previous publication acceptance boundary; it adds no authored flag,
annotation, host task registry or alternate execution backend. Embed retry
publication remains gated.

The existing shared canonical timer wait already handles parent cancellation.
Tests compose a retrying Split inside two published workflow agents and a root
workflow. Each published component must omit the lifecycle runtime import and
have no isolated invocation manifest or scoped-agent catalog. Cancellation after
the first HTTP/Slack error interrupts the 60-second backoff within the five-second
execution watchdog, acknowledges at the root and starts no further request or
recovery. The fixture schedules cancellation 250 ms after the response; it does
not instrument the timer-entry instant or establish a latency distribution.

Nine new emitted execution cases cover cancellation, successful ordinary retries,
exhaustion/recovery, zero retries, HTTP 429 and the existing loss of Slack
rate-limit classification across a Split boundary. Three also request parallelism
with two items. Split-level retries have an existing sequential fallback: each
failed attempt stops at its first failing item, and a successful attempt visits
both items. The tests assert that no parallel Agent pool is composed and preserve
these request counts. They do **not** demonstrate concurrent Split retries. A
barrier fixture requiring simultaneous requests could not complete, consistently
with the documented eligibility rule in `compile/split_parallel.rs`; it is not
kept as a purported cancellation test.

Qualifying the gate exposed a missing feature-analysis case: Split timeout
configuration was not recorded as `WorkflowFeature::Timeout`. It is now included,
including nested Splits, so callable retry checks cannot mistake those graphs for
runtime-free closures. The lowering tag advances to `retry-cooperation=v3` to
invalidate future compilation cache entries using the old import analysis;
existing parked artifacts retain their original execution contract. Boundary
coverage rejects retry publication for root/child durability, root/child logging,
explicit errors, signal waits, breakpoints and root/nested Split timeouts.

The emitted-component regression
`split_timeout_keeps_required_runtime_import_in_both_invoke_abis` additionally
checks zero and nonzero Split timeout configuration through both invocation ABIs,
validating actual component bytes and retained runtime imports. This preserves
the lower-level legacy callable compiler separately from the publication gate.

Validation with Rust 1.97 and isolated component/native directories:

- Normal component build: all 27 Agents and two shared workflow components.
- All nine new emitted execution cases passed. Full execution regression:
  358 passed, three manual benchmarks ignored.
- Workflow library: 577 passed. Native emitter audit: 30 passed.
- The complete root/published/composite retry module was rerun with the final
  persistence assertions: 29 passed, with no non-durable checkpoint writes or
  durable sleep calls on the checked paths.
- Feature-gated all-target Clippy for workflows/component host, formatting and
  `git diff --check` passed. The pattern lab generated all 98 scenarios after its
  explanatory text update.

No provider credentials, database/server E2E or fresh performance/capacity
measurements were used. This expands G2/G6 coverage; timer-only callable retry
failures, the complete construct matrix, timeout cancellation and the remaining
release gates still require their own qualification.

## Deadline selection contract before compiler integration (2026-09-08)

A composed WAT parent now exercises deadline-driven cancellation using the
existing production `runtara:host-io/timers.sleep` import and standard Component
Model waitable sets, polling, cancellation and drop. Its target is the actual
built HTTP Agent in the same Store. The sibling is test I/O; the `ready-barrier`
import only arranges readiness and cannot select an outcome or cancel a task.
This is an executable contract fixture, **not emitted DSL timeout support**.

The tested selection rules are:

1. Observe ready target/timer events before choosing. If target completion is
   ready in that observation, accept it even when the deadline timer is also
   ready. The test requires both actual completion events, not just an elapsed
   delay, and uses a readiness bitmask to prove the tie condition.
2. If only the timer is ready, select timeout, cancel the target through
   `canon subtask.cancel`, await its actual resolution and then drop its handle.
   A synthetic Agent proves that the callee may return a normal value during
   cleanup (`RETURNED`, rather than `CANCELLED`). That late value does not change
   the already-selected timeout outcome.
3. Cancel/drop an unused timer after successful target completion. Preserve the
   sibling, wait for its result and invoke the same target component again.
   Timeout cases keep the endpoint response pending until local socket closure;
   they do not wait for remote completion. The subsequent successful HTTP call
   demonstrates continuation and component reuse after cleanup.

Six real-HTTP cases cover pending headers, a partial body, completion before a
60-second deadline, both events ready, an already-due timer and `u64::MAX` timer
input. The zero value represents **zero remaining time**, not the semantics of
an authored `timeout: 0`. The maximum-wait case proves that this timer can be
cancelled without overflow; it does not prove absolute deadline arithmetic or
persistence. A seventh case uses a synthetic cancellation callback that returns
normally after releasing its native test request. Two mutation tests verify that
ending the readiness drain after one event or accepting the late cleanup result
violates the contract. All selection and task cleanup remain in the guest.

Fixtures live in `runtara-component-host/tests/cooperative_cancellation/`:
`deadline.rs`, `deadline-parent.wat` and `return-during-cancel.wat`. Existing Agent
composition tests reuse the same bounded parent-template/composition helper;
there is no per-provider implementation or product selector. The test transport
for the synthetic callback is deliberately separate from the six production HTTP
Agent proofs.

Next compiler work must carry a scoped deadline/reason through shared waits,
restore enclosing state after recovery, and route a timeout through the timed
step's existing error path. Root Cancel must still terminate the root, and a
selected local timeout must prevent ordinary retry in that expired scope. Define
budget start/zero behavior, queued-call expiry, persistence across durable retries,
inherited minimum budgets, CPU cooperation and independent emergency grace in the emitted tests
before retiring E128. The fixture's continuation call is not new authored
cancellation-handler syntax and does not qualify that complete DSL contract.

Fixture globals expose observations for assertions; compiler integration must
keep deadline state per invocation and scope alongside the shared wait locals.

Validation with Rust 1.97 on the final source:

- A clean `scripts/build-agent-components.sh` build produced all 27 Agents and
  both shared workflow components, including metadata.
- All nine deadline contract cases passed against those rebuilt components.
- Full component cancellation suite: 86 passed, including existing provider,
  synchronous/callback ABI and stalled-cleanup coverage.
- Feature-gated all-target component-host Clippy, formatting and
  `git diff --check` passed.

During the final checks, prior native/Agent build outputs disappeared and the
HTTP cases reported missing components. Source files remained intact. No caches
were removed by this task; components were rebuilt under a task-specific
`/private/tmp` target and the final suite was rerun successfully. The native check
directory was also rebuilt. No production emitter/runtime behavior or validation
acceptance changed; no fresh size/timing/capacity measurements or server/database
E2E were run for this contract stage.


## Structured Agent failures before scoped timeout integration (2026-09-08)

AUDIT-12 exposed a prerequisite for timeout recovery: the shared Agent failure
formatter converted structured WIT fields into a text prefix plus JSON. Embed
then failed to parse the result before reaching retry or recovery, while Split
lost the originating retry policy. The producer now emits a JSON object through
both the fresh-invocation and checkpoint-replay paths. Invocation context is
recorded as `stepId`, `agentId` and `capabilityId`; provider fields and attributes
remain structured. Invalid non-object attempt payloads are rejected explicitly.

Embed retains its own step identity, descriptive message and complete `childError`
context, while propagating the originating code, category, severity, explicit
retryability, retry delay and attributes. A missing child code still falls back
to `CHILD_WORKFLOW_FAILED`. The shared workflow retry classifier now respects an
explicit `retryable: false`, including cancellation-category errors. Root Cancel
still travels through the lifecycle path and cannot be caught by ordinary
`onError`. No host orchestration or new authored control is introduced.

This is a deliberate correction to failure behavior, not a claim of byte-for-byte
error compatibility. Newly compiled root Agent failures expose typed WIT error
fields instead of an empty code plus prefixed text. Embed handlers see the
originating code instead of always `CHILD_WORKFLOW_FAILED`. Existing compiled
artifacts contain their old stdlib and retain that behavior; the
`structured-agent-errors=v1` lowering identity separates newly compiled images.
Existing checkpoint key formats and raw Agent attempt envelopes are unchanged.
Historical text parsing remains only in the existing best-effort onError reader;
new execution paths do not parse a JSON substring out of an arbitrary message.

Verification covers shared envelope conversion, nested Embed propagation and
WIT projection, unsigned retry-delay preservation, explicit nonretryability,
permanent failures, malformed replay payloads, real HTTP/Slack errors through
composite retry waits, and durable Embed attempt replay.

- Normal component build: all 27 Agents and both shared components with metadata.
- Stdlib unit tests: 234 passed, one existing manual benchmark ignored, with both
  default features and `--no-default-features`.
- Compiler library: 577 passed. Native emitter audit: 30 passed.
- Real composed retry/cancellation cases: all 33 passed. This includes the four
  Embed cases that previously terminated at the JSON parse error, plus permanent
  recovery and zero-retry cases. Published Split still uses its existing
  sequential fallback when retry policy disallows parallel dispatch.
- Durable Embed/HTTP test: early restart preserves the exact checkpoint map and
  original wake with one HTTP request total; the due restart performs precisely
  one additional request and returns HTTP 200. Its initial fixture failure was
  a test-clock pin that had not been released; the corrected clock sequence
  follows the existing durable Agent replay fixture.
- Component cancellation suite: all 86 passed.
- Feature-gated all-target Clippy passed. All 98 illustration scenarios generated;
  the updated error-propagation view was inspected in the in-app browser.

- Full emitted-workflow suite: 363 passed, three manual benchmarks ignored.
  This includes existing Agent retry replay, onError routing, root cancellation,
  all audit execution cases and the newly reachable Embed retry paths.
- Formatting and diff checks passed. The mandatory commit hook additionally
  enforces workspace all-target Clippy and formatting before accepting the commit.

No new server/database E2E, resource soak or controlled performance comparison
was run for this stage. The existing WIT error-info fields carry the code,
message, category, severity, retryability, retry delay and provider attributes;
additional top-level invocation/child diagnostics remain in guest error context
and debug payloads rather than extending the WIT interface.

Agent/Embed step timeouts remain rejected by E128. This change provides their
shared error/recovery prerequisite; it does not implement deadline scheduling,
scoped timeout outcomes, cleanup grace, or complete the remaining plan gates.


## Published Embed retry waits and nested context restoration (2026-09-08)

Non-durable Embed retries now use the existing cooperative callable path when
all supplied root/child graphs require no lifecycle runtime. Publication safety
and runtime import omission share that closure check. An Embed reference alone
no longer forces a callable runtime import; each supplied child's features are
checked explicitly, including unexecuted recovery paths and deeper children.
Durability (including the default), logging, explicit Error events, waits,
timeouts and breakpoints keep runtime ownership. Missing, ambiguous and cyclic
child closures retain their existing safety diagnostics. Root workflow runtime
ownership is unchanged. The lowering identity advances to `retry-cooperation=v4`.

The common step-boundary helper previously emitted a consuming root signal read
unconditionally. Callable Embed completion now uses the shared canonical yield
and omits that read. This fixed the initial invalid-component failures at the
poisoned runtime index. There is no per-Agent exception, host graph dispatch,
new task interface, feature flag or child Store.

Nested tests exposed another shared-local lifetime bug: an inner Embed's error
branch reached the outer attempt before restoring the outer parent source. Error
wrapping then looked for the outer step in the child's graph (`unknown direct
step 'scope'`). The attempt frame now preserves parent source, child input, saved
entry data and retry checkpoint-key pairs. They are restored before wrapping,
checkpointing or retrying the failed child, including nonlocal error branches.
The existing outer frame still restores the context after the retry scope exits.

Tests exercise real HTTP and Slack failures through two published workflow
components, with both one and two inline Embed levels. Cancellation during
ordinary/rate-limit backoff returns through the root signal path with exactly one
HTTP request and no recovery/success publication. Normal retries preserve the
separate recognized rate-limit budget, zero retries still recover immediately,
HTTP_429 retains its existing ordinary budget, and permanent failures reach
recovery with their typed fields after one request. A nested HTTP case passes its
URL through both input mappings and verifies that subsequent attempts retain it.
Root nested Embed tests cover the same context repair outside publication.

A pure-child execution case covers default, zero and nonzero retries through the
same publication/composition helper. The child has no native Agent dependency;
its input/output mapping survives and no checkpoint or durable sleep is written.
This proves normal pure-child execution, not interruption during a failing
Agent-free callable backoff. Every published component is checked for omitted
runtime imports and absence of isolation selection/catalog artifacts. The source
compiler tests additionally validate pure and runtime-requiring child components
with zero/nonzero retries and exercise runtime ownership at three closure depths.

Validation completed:

- Normal component build: all 27 Agents plus stdlib/runtime and metadata.
- Compiler library: 579 passed; native emitter audit: 30 passed. The new safety
  matrix checks three closure depths, and emitted-component tests validate
  pure/durable/log/error/wait/timeout/breakpoint children with zero/nonzero retries.
- All 49 retry/cancellation scenarios passed, including 16 new published/nested
  Embed and pure-child cases. The pure-child test also covers the default retry
  policy in the full execution suite.
- The built-component callback/error contract test passed for all 27 Agents.
- Full emitted-workflow execution suite: 379 passed, 3 ignored, no failures.
- Feature-gated all-target Clippy, formatting and diff checks passed. All 98
  illustration scenarios generated and their new nested Embed test links resolve.

No server/database E2E, resource soak or new controlled performance report was
run in this stage. Agent/Embed timeout
E128, durable callable suspension, other remaining G1–G10 gates and fresh paired
performance measurements remain open. Retaining extra context uses the existing
guest operand-stack frame; binary-size and timing effects have not yet been
measured for this revision.

## Pure computation failures through published retries (2026-09-08)

A failing runtime-free child now reaches Embed retry and recovery. The new
reproduction uses only Finish input coercion: `data.count = "invalid-number"`
with an integer type hint. Before this change Split retried this plain stdlib
error, but Embed tried to parse it as JSON and returned a second failure,
`failed to parse EmbedWorkflow child error`, before its own retry or onError
handler. Published wrappers could therefore recover unexpectedly before a Cancel
notification arrived. Both failing Embed tests reproduced this behavior against
the preceding revision's components.

Both shared Embed error exports now accept JSON or plain error text. JSON remains
unchanged under `childError`; other bytes become a lossily decoded string there,
matching terminal error diagnostics. Plain failures retain the existing generic
`CHILD_WORKFLOW_FAILED` code and transient composite retry policy. Embedded JSON
fragments inside text do not become policy fields. Structured category, code,
retryability and delay propagation remain unchanged. This corrects the failure
contract for newly composed artifacts; the lowering cache identity includes
`plain-child-errors=v1`. No host task management or component ABI was added.

`cooperative_workflow_cancellation/pure_retry.rs` exercises both Embed and Split:

- Cancellation during a 60-second retry delay through two published workflow
  components, with a five-second execution watchdog. The child has no Agent,
  Error step or root runtime import. Acknowledgement occurs, recovery/terminal
  publication does not, and no checkpoint, durable sleep or event is written.
- Root and published recovery for zero retries, two delayed retries and two
  zero-delay retries. The output retains the actual integer-coercion failure;
  delayed cases verify that the configured waits were not skipped.

The cancellation fixture sends the signal 1.5 seconds after execution begins;
its finite child has no blocking work except retry timers. This is a backoff
interruption check, not a deterministic simultaneous-readiness race test or a
measurement of production cancellation latency. Normal delayed runs establish
that the failure enters backoff. Exact attempt counts are not instrumented in
these pure children; provider and Error-step fixtures cover counted attempts.

The stdlib unit matrix also covers plain/empty text, invalid UTF-8, JSON strings,
null, numbers, arrays and embedded JSON fragments through scoped/unscoped and
nested wrappers. Existing structured-error tests run through both exports.
The illustration adds a pure-child historical/fixed trace under AUDIT-12.

Validation: all components rebuilt; stdlib 235 passed, 1 ignored (plus
one doctest); compiler library 579 passed; all four new execution test functions
passed (14 executions); feature-gated Clippy passed. All 100 illustration
scenarios generate and all composite-error test links resolve. Browser inspection
confirmed the new trace, step progression and commands for both regression suites.
Full emitted-workflow execution: 383 passed, 3 ignored, no failures. The rebuilt
callback/error contract passed for all 27 Agents. No database or server E2E,
resource soak, or fresh size/timing comparison was run. Scoped
Agent/Embed timeouts, durable callable suspension and the remaining G1–G10 gates
are still open.

## Deadline outcome in the shared emitted wait (2026-09-08)

The production shared Await emitter now accepts an optional owned deadline
subtask and can return a distinct scoped-timeout outcome. This is an
internal compiler contract, not an authored option or alternate execution path.
It uses the same canonical async call status, waitable set, poll, cancel and drop
operations as the other cooperative waits. The owning scope will supply its
timer; the wait resolves that timer on every exit. No host task API or bookkeeping
was added.

The wait drains ready notifications before selecting expiry, including
nonterminal notifications. Completion wins if both operation and deadline are
observed ready in that batch. An observed root Cancel takes priority over timeout
recovery. Once timeout is selected, cancelling the operation cannot turn a late
normal return into accepted success. Deadline cleanup closes only this wait's
operation, polling timer and set. An enclosing active window remains live;
root/parent cancellation instead resolves the whole active window as before.

The deadline's packed status occupies one additional shared-helper parameter.
The helper now takes 16 i32 parameters, matching the first 16 canonical local
slots; no mixed-type parameter remapping or guest heap frame is needed. A second
local records the timeout selection at the caller. Normal/eager completion also
resolves an unused deadline. The standard non-cancellable `waitable-set.poll`
intrinsic is added for readiness draining; cancellable waits still deliver parent
cancellation. Cache identity advances to `cooperative-waits=shared-v2`.

Eight tests execute the actual generated Await function from a compiled workflow
module, with deterministic native fixtures for canonical events and resource
accounting:

- Completion/deadline readiness in either order, including nonterminal events.
- Selected timeout with cancellation resolving as returned, start-cancelled or
  cancelled; late writes cannot change the chosen outcome.
- Normal/eager completion with an unused pending deadline.
- Already-due deadline with or without an already-ready operation.
- Parent cancellation resolving the deadline, operation and active sibling.
- Unbounded waiting preserving its existing event path.
- Root Cancel observed before timeout recovery.
- Root polling timer cleanup while the unrelated sibling remains joined/live.

The fixtures reject double drops, cancellation before detachment and dropping a
set while handles remain joined. They export the real generated helper for
execution; they do not substitute a hand-written wait algorithm. These are core
emitter tests with simulated canonical events, not real Component Model I/O
interruption tests. The separate nine-test real-component deadline contract
continues to qualify those standard operations.

**DSL wiring remains incomplete.** Current workflow callers leave the deadline
input empty. They do not yet create a scope timer or route the timeout outcome
through Agent/Embed/loop recovery. Therefore this stage does not change E128,
activate interruption for existing loop deadlines, or prove inherited budgets,
parallel target ownership, durable deadline replay, cleanup grace or CPU
cooperation. Next, bind the timer to the owning scope's remaining budget, consume
the timeout outcome before result/retry/checkpoint handling, and restore the
parent scope before recovery. A deadline must not be consumed as an ordinary
Agent result or caught by an unrelated inner onError route.

Validation: compiler library 587 passed, including eight emitted-helper
tests and the shared-code growth bound; the four pure-workflow composed test
functions passed. Feature-gated Clippy and all nine real-component deadline
contract tests passed. Full emitted-workflow execution: 383 passed, 3 ignored,
no failures. Formatting and diff checks passed. No new binary-size/timing comparison, server/database E2E or resource soak
was performed. Extra helper state and cleanup branches still need the planned
paired performance measurement.


## Agent deadline integration behind the existing support gate (2026-09-08)

Sequential Agent lowering now connects the owning invocation/retry budget to the
shared Await helper. This is production emitter code exercised through a private
emission seam in tests; public compilation still rejects Agent/Embed `timeout`
with E128. There is no product feature flag or opt-in annotation.

The manifest retains the authored timeout instead of discarding it. Static Agent
metadata supplies the budget to the common invocation path. After mapping,
validation, and the result-cache probe, a cache miss initializes one absolute
budget. Non-durable steps keep it in guest locals without checkpoint writes;
durable steps persist eight bytes under an `agent-deadline` identity that includes
workflow, invocation ancestry, graph scope, and step. Failed-attempt replay and
retry wake records share this budget. A successful result-cache hit bypasses its
old deadline. Existing loop identities and completed-loop behavior are unchanged;
the existing deadline-key export is shared rather than adding an Agent task API.

Before an attempt, an expired budget produces typed `AGENT_TIMEOUT` without
sending a request. A live attempt starts an owned timer in the existing standard
waitable set. After timeout selects cancellation and resolves the subtask, the
Agent caller overwrites any late return with `AGENT_TIMEOUT`, category `timeout`,
`retryable: false`. Existing `onError` receives this structured error; an unhandled
one becomes a typed workflow failure. Root cancellation retains its suspend/ack
path and cannot enter the ordinary recovery route. Retry sleep durations and
persisted wakes are capped by the remaining budget; an expired replay cannot
send another request. Saturating addition and subtraction handle zero and u64
bounds. Malformed durable budget widths fail with `AGENT_DEADLINE_STATE`.

Tests in `compile/agent_deadline_tests.rs` run the actual emitted, statically
composed HTTP Agent and stdlib components. They cover pending-header cleanup,
zero timeout with retries, positive timeout with/without retries, backoff expiry,
early and expired durable replay, successful cached replay, u64::MAX, a later
hanging attempt using a reduced original budget, root cancellation with an active
deadline, unhandled typed failure, and malformed checkpoint widths. The later
attempt test advances the fixture clock before its retry and bounds elapsed time
from the first HTTP request; restarting a full relative budget fails that check.
CI explicitly runs the feature-gated library deadline suite after building the
component bundle; the ordinary workflow integration target would not run these
private-emitter tests. The loop/Agent key test separately checks attempt independence and separation
across graph scope, loop invocation, and published-child namespaces.

Qualification limits remain material:

- This stage uses the existing runtime wall clock. Runtime-free published
  workflows and a monotonic in-run budget, including clock rollback handling,
  still need clock lowering and tests. The private emitter rejects a missing
  clock lowering rather than emitting a poisoned import.
- The budget currently starts after mapping/validation/cache lookup. Connection
  preparation consumes elapsed budget, but this stage does not interrupt a
  blocked connection resolver or arbitrary synchronous preparation.
- Inherited loop/Embed deadlines are not yet raced against this I/O. Their owner
  and unwind/recovery target must be preserved so an inner Agent handler cannot
  swallow an outer expiry. Nested restoration needs composed qualification.
- Timed Agents are excluded from speculative parallel launch while scoped timer
  ownership is implemented. E128 keeps this internal fallback from becoming a
  released parallelism change. Parallel sibling survival still needs DSL proof.
- Cleanup uses the existing synchronous canonical subtask cancellation contract;
  a noncooperating operation can stall it. Scoped cleanup grace and independent
  emergency escalation are not supplied by this patch.
- Capability transport timeouts and AiAgent turnTimeout retain their own behavior;
  AI auxiliary calls and Embed timeout lowering are not added here.

Cache identity advances to `cooperative-waits=shared-v3`. The emitted core adds two
i64 locals; timeout error data is emitted only when a manifest contains a timeout.
No new host bookkeeping, graph interpretation, child Store, task registry, or
custom cancellation import is introduced. This is not a performance comparison;
paired size/timing/resource measurements remain outstanding.

Verification:

- Compiler library with the integration-test feature: 597 passed, including the
  eight composed deadline test functions.
- Stdlib: 236 passed, one existing ignored case, and one doctest passed.
- Full direct workflow execution target: 383 passed, three existing ignored
  benchmarks, zero failures (503.08 seconds).
- Normal component build: all 27 Agents and both shared components, with metadata.
- The eight deadline test functions passed again against the final rebuilt
  bundle; the all-27-Agent callback/error contract passed too.
- Feature-gated all-target Clippy for workflows/stdlib, formatting, and diff
  whitespace checks passed. The mandatory workspace pre-commit hook also runs.

Server/database E2E, resource soak, and fresh paired measurements were not run
for this stage. No release or PR completion is claimed.


## Standard monotonic clocks and published Agent budgets (2026-09-08)

Running Agent invocation/retry budgets now use
`wasi:clocks/monotonic-clock.now`. The emitter parses the repository's resolved
standard clock and poll WIT inputs, exposed by the lightweight `runtara-agent-wit`
crate; it imports only the clock's `now` function into the core module. No resolved
WIT dependency files are edited and no parallel handwritten clock contract is
introduced. Clock requirements are derived from the manifest and retained in
returned/re-emitted scaffolding. Untimed workflow scaffolding keeps its existing
imports; there is no authored flag or annotation.

A live scope stores a monotonic start instant in nanoseconds and a budget duration
in milliseconds. Remaining time subtracts elapsed whole milliseconds from that
budget with saturation. It does not multiply a u64 millisecond budget into
nanoseconds or add it to the clock's unspecified origin, which would lose the
large-duration range. Millisecond timer granularity is retained. Two additional
i64 locals carry the live clock state; cache identity is `cooperative-waits=shared-v4`.

Non-durable scopes need no lifecycle clock or checkpoint I/O. Durable scopes
retain their existing eight-byte epoch deadline for replay and scheduler wakes,
sample its remaining duration when entering a live invocation, and then use the
monotonic clock for that invocation's remaining budget. A recovered duration is
capped at the authored timeout. Active clock jumps cannot restart or shorten the
live timer; elapsed time across a park/restart still depends on the existing
epoch-clock/scheduler contract. A monotonic origin is not persisted across
Stores or processes. This does not claim immunity to arbitrary clock corrections
while an instance is parked.

Deadline metadata now traverses every inline nested graph as well as supplied
child-workflow graphs. The prior first-stage inventory only inspected the outer
graph of each supplied definition; an Agent inside a While/Split definition was
missing. The common inventory feeds both static Agent data and required clock
imports, so an inline Agent cannot silently lose its timer.

Composed qualification expands to one and two published-workflow layers. Each
child artifact is decoded to verify standard clock imports, absence of a lifecycle
runtime import, and absence of an isolation catalog. The final root receives
signals normally. The test matrix covers hanging HTTP, zero timeout, retry
backoff expiry, u64::MAX success, and root Cancel traversing the child chain.
WAC may unify compatible WASI patch versions with the Rust-produced bundle; the
resolved emitter input is 0.2.3 and the current composed bundle exposes 0.2.9.

Separate root tests move only the fixture's epoch clock forward/backward during a
failed first request. A later hanging attempt still ends against the original
two-second live budget. A one-second retry delay makes a per-attempt timer reset
observable. Actual emitted-arithmetic tests cover near-u64::MAX monotonic origins,
full u64 millisecond budgets, sub-millisecond elapsed time, equality and expiry.
One/two inline While levels exercise the missing-inventory fix with timeout,
success, and ordinary HTTP error. An onError Finish keeps the pre-existing terminal
workflow behavior; these cases do not claim recovery that continues the enclosing
loop. The existing CI deadline command also runs the new composed cases.

The public E128 gate remains. Inherited scope ownership/unwind, parallel timers,
interruptible preparation, Embed/AI scope coverage, and bounded cleanup grace
remain required before enabling Agent/Embed timeout syntax. No host task manager,
new cancellation import, lifecycle bookkeeping, or per-call Store was added.
The existing awaitable timer remains the owned deadline waitable.

Verification on the pinned toolchain and freshly rebuilt components:

- Normal component build: all 27 Agents and both shared components, with metadata.
- Full feature-gated compiler library suite: 600 passed, including the composed
  deadline cases and emitted monotonic arithmetic.
- Full direct workflow suite: 383 passed, three manual benchmarks ignored.
- All-27-Agent shared callback/export/error contract: passed against the rebuilt
  bundle.
- Feature-gated Clippy for `runtara-workflows` and `runtara-agent-wit`, all targets,
  with warnings denied: passed.

No new latency/size benchmark, resource soak, or server/database E2E is claimed in
this stage.


## Inherited deadline ownership and sequential unwind (2026-09-08)

While/Split budgets now participate in a guest-owned enclosing scope. The emitter
selects the earliest remaining monotonic duration, remembers its manifest-wide
loop owner, and carries its static timeout payload through unwind. Equal remaining
budgets retain the outer owner. An Agent's own timeout participates in the same
selection; a shorter Agent budget still reaches that Agent's normal error path.
Standard Component Model Await cancels and resolves the selected pending call.
No host graph interpreter, task registry, new cancellation import, or child Store
was added.

Loop and Embed frames preserve the active scope; Split aggregation also snapshots
it in its existing failure frame. A selected enclosing reason bypasses child
Agent retries/handlers, ordinary item aggregation, and Embed retry checkpointing.
The owner consumes that reason only after restoring its parent scope and before
running recovery. Untimed inner loops check the enclosing budget at cooperative
boundaries. This covers DSL loops, not arbitrary non-cooperating CPU code inside
an Agent capability.

Running loop time now follows `wasi:clocks/monotonic-clock.now`. Epoch deadlines
remain eight-byte durable records for replay and scheduler wakes. Entering a live
scope reconstructs a duration capped at its authored timeout. Completed scopes
are not reinstalled. Retry sleeps intersect the enclosing live duration; the
common retry-park lowering clamps the persisted wake to the enclosing epoch
budget before saving it. Time spent parked still depends on the epoch/scheduler
contract. Existing zero-disabled While/Split timeout syntax is unchanged. Cache
identity is `cooperative-waits=shared-v5`.

The new tests exposed two error-routing defects: durable Agent recovery omitted
the open checkpoint branch from its branch depth, and consuming a nested failure
through Split aggregation/retry could leave the shared fatal-error flag set. Both
are fixed. Durable Split bodies now account for their open cache-miss branch too.
Loop frames also retain their own parent steps: a hanging-child regression first
returned the inner scope's prior output from the outer handler. Distinct prior
values now survive live timeout unwind in all four While/Split nesting pairs.
The final-body-overrun test now delays an actual debug callback; a separate test
advances only the epoch clock and requires normal completion. This distinguishes
elapsed work from clock correction instead of using a clock jump to simulate work.

Composed qualification covers one/two While levels, shorter Agent/inner budgets,
durable and non-durable child Agents, zero-retry and retry-enabled recovery,
sequential Split aggregation with/without retries, ordinary HTTP error/success,
root Cancel, and inherited backoff with early/expired durable replay. A public
Agent-free workflow holds epoch time fixed, times out an untimed CPU loop, then
runs another untimed loop in recovery to prove the old scope was removed.
Production I/O tests also cover AI single-shot, chat-turn, memory-load,
summarization and memory-save calls, and two nested Embed layers during pending
headers or response body. They require the enclosing timeout handler, socket
cleanup, no extra requests, no child-attempt checkpoint for enclosing cancellation,
and no acknowledgement of a nonexistent root command.

Live parallel windows and sibling preservation still need inherited timer
qualification. Own Embed/AI/tool deadlines, all preparation/retry wait sites,
cleanup grace/non-cooperation escalation, public server E2E, resource soak and
controlled paired performance measurements remain open. AI tool propagation is
wired before tool-result feedback, but this stage's provider I/O matrix does not
independently qualify every tool-arm shape. E128 stays in place; no complete
cancellation rollout is claimed.

Verification on the final source:

- Compiler library tests: 605 passed.
- Feature-gated direct workflow execution suite: 387 passed, three manual
  benchmarks ignored.
- Feature-gated workflows Clippy, Rust formatting and `git diff --check`: passed.
- Updated patterns page JavaScript: syntax checked with Node.

This stage reuses the previously built Agent/stdlib/runtime bundle; none of those
component sources changed. No new component build, visual browser inspection,
size/latency benchmark, resource soak, or database/server E2E is claimed here.


## Parallel scope deadlines and launch boundaries (2026-09-08)

The shared `WindowWait` helper now observes an enclosing deadline alongside the
existing lifecycle poll timer and parallel call handles. The entry function
selects the earliest guest-local scope; the helper manages the standard waitable
set and resolves owned handles. A selected deadline cancels the active window,
drops its calls/timers/set, releases that window's pause/shutdown deferral, and
returns the existing timeout outcome. Recovery then follows the same owner-aware
unwind as sequential work. Root Cancel is checked before selecting timeout.
No host task registry, graph dispatch, extra Store or cancellation WIT was added.

Ready call completions are delivered before deadline selection regardless of
notification order. Timer completions never count as item completions. Each wait
resolves its deadline timer before returning an ordinary event; branch assembly
can then await other I/O without overwriting or inheriting a live timer. This
creates a timer per scoped wait, rather than retaining one across assembly. The
controlled size/latency comparison must include this cost; no performance claim
is inferred from correctness tests.

A shared `WindowCancel` helper handles expiry observed around launch/assembly
boundaries without waiting for peers. These checks prevent a fast branch from
starting its next call after the enclosing scope expires. Parallel preparation
is checked before entry and again before Agent invocation; these checks do not
yet interrupt a blocking preparation import while it is running. Split also
checks between windows and after final assembly. Cache identity is
`cooperative-waits=shared-v6`; helpers retain the existing 16-value state transfer.

A Split's own timeout no longer forces sequential fallback. Other eligibility
rules remain: Split/Agent retries, Agent-owned deadlines, workflow-agent bodies
and breakpoint shapes still have their current restrictions. W073 and the DSL
field documentation now describe the implemented timeout behavior. This does
not declare targeted per-Agent sibling preservation or the full G6 gate complete.

New coverage executes the real emitted helpers with deterministic event ordering
and normally composed workflows with production HTTP I/O:

- Four helper tests cover ready/deadline ties (including already-due and STARTED
  events), all three legal cancel resolutions, root/parent cancellation priority,
  lifecycle timer cleanup, exact handle ownership and balanced deferral.
- Enclosing While expiry cancels live Split windows, independently advancing
  branches, depth-wavefront branches, and parallel calls inside two Embed layers.
- Split-owned expiry retains two overlapping calls and cleans both pending
  headers or partial response bodies before the handler completes.
- Success controls enforce overlap with a two-request barrier, complete in
  reverse order, retain input-order results, and replay a durable result without
  sending another request. Ordinary HTTP 503 still reaches normal recovery in
  durable and non-durable workflows.
- A fast branch completes one request, spends 600 ms in a debug callback under a
  500 ms enclosing budget, and never sends its next request. Its pending peer
  closes before the enclosing timeout handler completes.

Verification on the final source: 609 compiler library tests and 392 direct
workflow execution tests passed (three manual benchmarks ignored), with
`direct-wasm-integration-tests` enabled. Feature-gated all-target workflows Clippy,
Rust formatting, `git diff --check` and patterns-page JavaScript syntax passed.
The Agent/stdlib/runtime sources and WIT did not change; the suite reused the
previously rebuilt component bundle. No fresh component build, browser visual
inspection, database/server E2E, soak or controlled size/latency run is claimed.

Own Embed/AI/tool budgets, interruptible preparation, individual timed-Agent sibling
survival, cleanup grace/non-cooperation, authenticated server/multi-owner E2E,
resource soak, controlled paired measurements and superseded-path retirement
remain open. E128 stays in place for Agent/Embed timeout syntax.

## Interruptible connection preparation (2026-09-08)

All 27 Agents retain the shared `agent_component!` export/dispatch macro. This
stage moves connection-metadata waiting onto the same emitted cooperative wait
used by invocation. `agent_io::emit_connection_description` is shared by the
sequential Agent/AI path, Split preparation and branch preparation. It arms the
earliest applicable budget and resolves the pending lookup before returning a
timeout. Callers skip descriptor injection and Agent invocation on that result;
parallel preparation also resolves the enclosing window before propagating its
selected scope's timeout. No custom host task management is added.

The resolver WIT is now async-typed 0.2.0 for both `describe` and
`resolve-resource`, with concurrent host bindings. Existing 0.1.0 binaries keep
their synchronous host bindings and use the same resolver implementation and
per-run caches. This version change is required because the async function kind
is part of the Component Model type; changing a binding under the old interface
would break linking. New compilation uses 0.2.0 without feature selection.
The compiler cache identity advances to `cooperative-waits=shared-v7`.

Preparation coverage includes root Cancel and enclosing While expiry during
metadata headers and partial bodies across sequential Agent, AI, two inline
Embed layers, an actual published workflow-agent, Split, scheduled branches and
wavefront branches. Parallel fixtures require a live peer before metadata blocks.
They forbid actual Agent invocation, retry checkpoints and failure publication.
The published-child budget belongs to its calling parent: existing validation
of runtime-bearing published graphs has not been weakened for the fixture.
Private-emitter tests additionally cover own Agent budgets, zero budgets, root
Cancel priority and durable/non-durable execution. E128 remains public policy.

A production-linker fixture executes old and new resolver ABIs, calls both
resolver functions twice and requires exactly one HTTP request per operation,
preserving results and the per-run cache contract. The normal AI connection
resolution control also passes. All 27 Agent components and both shared workflow
components were rebuilt successfully with metadata using the pinned toolchain.

This does not qualify arbitrary preparation work or CPU transformations, own
Embed/AI/tool budgets, targeted timed-Agent sibling preservation, remaining
cleanup-grace cases, authenticated server/multi-owner E2E, resource soak or
controlled paired size/latency measurements. Those plan gates remain open.

Verification: 610 compiler library tests pass. The strengthened preparation
matrix passes all 28 cases in three tests, observing socket closure inside the
live recovery callback or cancellation acknowledgement. Ten private-emitter own
Agent preparation cases pass as part of the compiler suite. The feature-gated
real-component cancellation suite passes all 86 tests, including shared export
and error-contract coverage for all 27 Agents. Resolver compatibility passes for
both ABI versions; 60 macro tests and six WIT tests pass (four macro doctests are
intentionally ignored). Feature-gated all-target workflows/component-host Clippy,
formatting, diff checks and patterns-page JavaScript syntax pass. No browser
visual check, database/server E2E, soak or controlled benchmark was run here.

The full `direct_wasm_execute` integration suite also passed: 395 tests, with
three manual benchmarks ignored. After strengthening the recovery-time cleanup
assertion, its three preparation tests were rerun and passed; the final compiler
suite and feature-gated Clippy run include that fixture revision.


## Embed-owned cooperative budgets (2026-09-08)

The ordinary inline `EmbedWorkflow` run-plan path now initializes its own total
budget on a result-checkpoint miss. It reuses Agent budget arithmetic,
`deadline_scope` ownership and the standard cooperative waits. Child attempts
share the same deadline; nested frames preserve it. The owning Embed resolves
its child work, restores the parent scope and routes `EMBED_TIMEOUT` with
`retryable: false`. An earlier enclosing deadline propagates past the Embed's
handler instead. A zero budget skips the child, and successful checkpoint replay
bypasses an expired budget.

Durable Embed budgets use an eight-byte epoch deadline under the existing
structured key scheme with the new `embed-deadline` kind. The shared stdlib key
function retains its old WIT name and preserves Agent/loop keys. Definition,
invocation and namespace identities stay separate; retries reuse the same
budget. Malformed state returns `EMBED_DEADLINE_STATE`. Live elapsed time uses
WASI monotonic clocks; durable Delay/retry wakes are clamped to the scope.

This also fixes Embed retry backoff under an inherited scope: the shared wait
clamp bounds the sleep, and the post-wait boundary checks expiry before starting
another child attempt. Own expiry bypasses retry checkpointing and child
recovery. No host scope registry, task API, additional Store or product switch
is introduced. The compiler cache identity is `cooperative-waits=shared-v8`.
New Embed error strings are included only in artifacts containing an Embed
budget; existing local slots and scope frames are reused.

Eight private-emitter tests in `compile/embed_deadline_tests.rs` exercise zero
budgets, hanging headers/partial bodies, root Cancel, ordinary failures, success,
u64 saturation, completed replay, non-durable retry sleep, durable retry parking,
early Delay resume, malformed deadline state, absent recovery handlers, nested
Embed retries, two overlapping Split child calls, and both parent/child deadline
orderings. Recovery debug callbacks require socket closure while the workflow
is still running. The tests supply the complete child closure and assert the
specific support rejection and validator E128 before privately emitting the
candidate, so they introduce no production opt-in.

The final compiler regression run passed 618 tests; the final strengthened Embed
matrix passed all eight tests. The shared stdlib suite passed 237 tests with one
existing ignored test. All 27 Agent components and both shared workflow
components rebuilt with metadata; the feature-gated real-component cancellation
suite passed all 86 tests. These are execution/compatibility results, not new
performance measurements.

E128 remains in place. This qualifies the ordinary inline Embed run-plan path;
Embed-as-AI-tool budgets, runtime-free publication with owned Embed deadlines,
full completion-race/cleanup-grace qualification, other preparation gaps,
individual timed-Agent sibling preservation and the remaining G1–G10 gates are
still open. It does not complete the cancellation plan or its E2E, soak and paired
measurement requirements.

Final integration verification: `direct_wasm_execute` passed all 395 tests
(three manual benchmarks ignored). Feature-gated all-target workflows/stdlib
Clippy, Rust formatting, `git diff --check` and patterns-page JavaScript syntax
passed. The final eight-test Embed rerun includes the explicit E128 assertion
and child-aware support analysis. No browser visual check, database/server E2E,
resource soak or controlled size/latency run is claimed for this stage.

## Inline Embed tools: shared scopes and budgets (2026-09-08)

`emit_embed_workflow_tool_arm` now calls the same budget entry, exit and error
capture helpers as the normal Embed path. Its run-plan retains the authored
timeout and durability fields. The additive stdlib `tool-scope-source` export
reuses the existing workflow-agent tool identity calculation while preserving
all caller source fields. It scopes the child, completed result and budget to
one AI step/label/call-counter invocation. Existing published workflow-agent
scope formulas are unchanged.

A completed durable result bypasses budget loading; a pending call retains its
absolute epoch deadline through early resumes. Own expiry is wrapped as
non-retryable `EMBED_TIMEOUT` feedback after restoring the parent deadline.
Inherited expiry skips result caching and propagates out of the AI loop; root
Cancel follows the existing signal acknowledgment/lifecycle path. No additional
host cancellation registry, Store, host interface or feature switch is added.
The compiler cache identity advances to `cooperative-waits=shared-v9` for the
changed guest import and emission.

Compatibility: newly compiled inline Embed tools use per-call checkpoint
namespaces instead of their previous shared child namespace. Previously composed
artifacts keep their embedded implementation; this stage does not migrate parked
instances to a newly compiled binary.

Seven composed test functions cover zero budgets, repeated HTTP calls in both
success/timeout orders, hanging headers and bodies, root Cancel, both parent/own
expiry orderings, ordinary error/success, completed replay and a partially
completed turn parked on Delay, plus malformed pending budget records. Two new native tests exercise malformed source
shape and context preservation with legacy/v2 tool identities and a large
payload. The private timeout fixture still asserts public E128 rejection with
the complete child closure before emitting its candidate.

The pending-turn replay fixture repeats the model response. Persisting that
response before tool dispatch remains necessary for nondeterministic model
replies, and nested AI-tool state/arena frames still require qualification.
E128, the remaining G1–G10 gates, server E2E, soak and paired measurements remain
open. No new performance result is claimed here.

Verification: all 27 Agent and both shared workflow components rebuilt. The full
compiler suite passed 624 tests; the final seven-test tool matrix then passed
with exact feedback assertions and the additional corrupt-budget case. Stdlib
passed 239 tests (one existing ignored), plus its doctest. All 86 real-component
cancellation tests and all 395 workflow integration tests passed (three manual
benchmarks ignored). Final feature-gated all-target Clippy, formatting, diff and
patterns-page JavaScript syntax checks pass. Browser visual checks,
database/server E2E, soak and controlled benchmarks were not run for this stage.

## Persist AI decisions before tool dispatch (2026-09-08)

The shared durable AI loop now loads or saves each successful `chat-turn`
response under a separate `ai_turn_response` checkpoint. Saving happens before
any tool dispatch and uses the existing checkpoint/signal machinery. A pause at
that write leaves the decision durable and every tool unstarted. A pending-turn
resume uses its original response without a provider call; its completed-turn
snapshot is still written after tool results are assembled. Non-durable loops
retain their authored behavior. The response cache hit is validated but is not
rewritten.

The stdlib's additive response-key and response-validation exports preserve the
existing completed-turn key formula. Validation rejects corrupt decisions with
`AI_TURN_RESPONSE_STATE` before model or tool I/O. It permits unknown fields and
argument values and bounds the stored iteration/tool-index integers to their
wire representation. The compiler reuses existing local slots, adjusts error
unwind depth through the cache-miss branch, and advances its artifact cache tag
to `cooperative-waits=shared-v10`. No host interface or task registry is added.

This also fixes the shared checkpoint lookup/save helpers: failed reads no longer
become cache misses, and failed writes no longer allow execution to continue.
The storage error terminates execution. This cannot reverse an HTTP side effect
whose result failed to save; external idempotency and interruption before model
response persistence remain separate concerns.

The twelve composed tool tests now include both read/write storage faults, a
pause immediately after response persistence, malformed saved responses, a
mixed Agent/WaitForSignal turn with repeated early resume and exact tool IDs,
and generic Agent checkpoint faults. The earlier parked-Embed proof now permits
only the initial decision and final answer provider calls, rather than scripting
the same decision repeatedly. Native tests cover key separation, replay identity
and malformed/large response shapes. E128 and the remaining G1–G10 gates stay
open; nested AI tool frame/arena behavior still needs qualification.

Cost: a fresh durable turn adds a response-checkpoint lookup/write and stores its
response separately from the completed-turn snapshot. Resume avoids a provider
call for an already persisted pending turn. The measurement plan now includes
1/10/100-turn AI loops, checkpoint bytes/latencies and growing conversations.
No new benchmark, database/server E2E or soak result is claimed at this stage.

Scope of the storage fix: this stage changes the shared lookup/save helpers.
Raw attempt-replay reads in `agent.rs` and `split_parallel.rs`, debug checkpoint
handling and deferred parallel-launch failures still need dedicated fault and
peer-cleanup qualification. The plan records that inventory; this stage does
not establish storage-failure correctness for every emitted call site.

Verification: all 27 Agent and both shared workflow components rebuilt. The full
compiler suite passed 628 tests; the final 12-test tool matrix also passed after
adding generic checkpoint faults and exact mixed Agent/signal-tool replay.
Stdlib passed 241 tests (one existing ignored) plus its doctest. All 86
real-component cancellation tests and all 395 workflow integration tests passed
(three manual benchmarks ignored). Final feature-gated all-target Clippy,
formatting, diff checks and patterns-page JavaScript syntax passed. No browser
visual check or additional performance/E2E/soak result is claimed for this stage.

## Preserve AI callers across inline Embed children (2026-09-08)

The ordinary Embed attempt frame now preserves AI-loop context through shared
`push_child_frame`/`pop_child_frame` helpers. They save the existing AI locals on
the guest operand stack and restore them after the child attempt, including a
captured child failure. The designated result locals are excluded. This keeps
outer pending results, conversation, tool indices/counter, iteration and heap
watermark independent of an inner AI loop. It introduces no new host import,
Store, runtime task state or canonical local slots.

Tool planning also now checks the current graph's declared step type before
resolving an Embed child. Reusing an outer Embed ID for an inner Agent previously
caused unbounded planner recursion and native stack overflow; after that fix,
the execution reproduction separately showed the first of two child results
being lost. Both failures were observed before their corresponding fixes. The
new planner check also covers Wait tools with the reused ID; existing true
child-closure cycle rejection remains intact.

The six composed tests in `compile/nested_ai_tests.rs` verify multi-turn and
multi-tool caller preservation, large histories, ordinary child failure,
root/own/parent cancellation with pending model headers/body, nested signal
resume, completed replay and child arena collection with a large outer value.
They cover durable/non-durable execution as applicable and use the existing
private-emitter E128 assertion for authored Embed budgets. The compiler artifact
cache identity is `cooperative-waits=shared-v11`.

Other inline callbacks and mixed recovery/parallel contexts remain to qualify.
The frame's extra saved values/code must be included in paired measurements;
this stage makes no new benchmark claim and does not complete G1–G10 or retire
E128. See AUDIT-13 for the concrete failure and test mapping.

Verification: all 27 Agent and both shared workflow components rebuilt. The final
compiler suite passed all 636 tests with `--test-threads=1`; the focused nested-AI
matrix passed all six tests. All 395 workflow integration tests passed (three
manual benchmarks ignored), as did all 86 real-component cancellation tests.
Feature-gated all-target Clippy, formatting, diff checks and patterns-page
JavaScript syntax passed.

The first compiler run, concurrent with integration/component workloads, had
three older 200 ms deadline fixtures expire before their first HTTP request
arrived. A targeted retry case also failed while the integration process was
still busy; the inventory recheck passed. The complete serial rerun passed with
the original budgets and assertions unchanged. These results establish functional
regression coverage, not a timing guarantee under contention. Browser visual
checks, database/server E2E, resource soak and paired performance were not run
for this stage.

## Centralize checkpoint failures and owned-window cleanup (2026-09-08)

Every raw emitter checkpoint read/write now passes through the shared helpers in
`compile/checkpoint.rs`. An additional shared core failure function saves the
canonical error fields before resolving outstanding handles through the existing
standard cancellation helper, then restores and reports the original error. It
uses the existing guest state/locals and avoids lifecycle polling that could
replace the storage diagnostic. The artifact identity advances to
`cooperative-waits=shared-v12`; no agent WIT, host implementation, component
packaging or per-agent adapter changes.

The three initial regressions failed by returning Completed: an unreadable
attempt checkpoint, a transient parallel prelaunch lookup and a failed breakpoint
write. Six final test groups also qualify failed attempt writes, queued Split and
branch calls, and a pending branch peer at both HTTP headers/body waits. The peer
must close before `runtime.fail` returns, while the Store is still alive.

Retrying Split items retain sequential fallback, so parallel attempt-read tests
use zero retries to exercise the actual window. Storage failures remain terminal;
these tests do not establish sibling-preserving timeout recovery, malformed
checkpoint payload handling, deeper mixed frames or cleanup grace. The original
G1–G10 completion requirements, benchmarks, E128 and remaining E2E/soak work stay
open. See AUDIT-14 for the case-to-test mapping.

Verification: the final serial compiler suite passed **642 tests**, and the
workflow integration suite passed **395 tests** (three existing manual benchmarks
ignored). Feature-enabled all-target Clippy with `-D warnings`, formatting,
diff checks and patterns-page JavaScript syntax passed. The first full compiler
run exposed 12 helper tests that assumed Await/WindowWait were the last emitted
functions, plus one test that confused the error string length with a cached
payload pointer. The harness now uses registry positions and the cached
pointer/length pair; the complete rerun passed without weakening behavioral
assertions. Existing agent/shared component artifacts were reused because their
source and WIT did not change. No new paired measurements, browser visual checks,
database/server E2E, standalone component-host suite or soak ran in this stage.


### Parallel Agent deadline lowering · 2026-09-08

The guest's existing parallel slots now carry each timed Agent's original
monotonic start/budget, ready state and timeout error descriptor. One standard
async timer selects the nearest active Agent deadline. Ready call handles remain
owned by their slots until the normal scheduler drops and assembles them; the
shared cleanup resolves them on root/parent cancellation or scope failure. A
memoized result does not restart its Agent budget during assembly.

Branch dispatch and memoized assembly now adjust outer error/handled branch
depths consistently. A scheduler error capture reuses the shared window cleanup
before forwarding an ordinary failure to an outer handler, and avoids repeating
cleanup when an enclosing deadline already resolved the window. This fixes an
HTTP failure that previously reached recovery with a live peer request.

Six composed regression groups cover pending HTTP headers/body with a surviving
peer, a fast three-call sibling chain, ordinary-error cleanup before recovery,
zero/maximum budgets and completed replay, parallel Split timeout aggregation,
and a surviving Split item after the earlier item spent budget on connection
preparation.
See AUDIT-15 for exact test names and pending qualification. Public E128 remains;
this is not a completed timeout-release or bounded-grace gate. No agent component
or host protocol changed in this stage. Parallel slots grow from 176 to 208 bytes,
shared helper state from 16 to 19 i32 values, and the emitter cache marker advances
to `cooperative-waits=shared-v13`. Paired size/latency and soak measurements remain
required, including the untimed path.


Validation for this stage: the full feature-gated compiler library suite passed
647 tests in 188.60s. After adding the final Split preparation/survival fixture
and extending its shared test server, the affected checkpoint/parallel group
passed 13 tests in 10.45s; no production code changed after the full library run.
The workflow execution integration suite passed 395 tests in 527.92s, with its
three manual benchmarks ignored. The macro crate passed 60 tests (four example
doctests remain ignored). Final feature-gated Clippy, formatting, diff whitespace
and the interactive guide's JavaScript syntax checks passed.

Agent components were reused from the existing rebuilt set: this stage changes
no guest Rust agent, WIT or component-host source. The standalone component-host
suite, authenticated server E2E, Linux run, visual browser review, paired size/
latency measurements and soak were not rerun here. Existing publication, cleanup
grace and compatibility-retirement gates remain open.


### Service parallel deadlines inside Await · 2026-09-08

The existing Await helper now shares the window's waitable set when a timed
parallel window is active. It services the window's nearest Agent deadline while
waiting for connection preparation, buffering returned/expired peer outcomes in
their existing slots. On exit it resolves its own calls/timers while retaining
the shared set for the window. Root/parent cleanup owns all calls. No peer-handle
transfers or additional preparation sets are required. A closed window clears
its timed flag before later waits can reuse scratch memory.

Four composed regressions cover earlier Agent expiry, successful peer return,
root cancellation and enclosing Split timeout while a later lookup is pending.
The root acknowledgement and parent failure callbacks require all fixture
sockets to close before reporting, while the Store still exists. The fixture
runs both pending headers and partial bodies. See AUDIT-16 for names and limits.
No agent/host/WIT changes or new state fields were required. Shared helper count
and state width remain seven and 19; the emitted cache marker is now
`cooperative-waits=shared-v14`. Performance and cleanup-grace qualification remain
open.


Final validation for this stage: 652 feature-gated compiler library tests passed
in 213.86s, and 395 workflow execution integration tests passed in 559.38s (three
manual benchmarks ignored). Feature-gated Clippy, formatting, diff whitespace and
the interactive guide's JavaScript syntax checks passed. This includes the four
new preparation regressions and the existing deterministic wait-event tests.

The existing sequential pending-HTTP fixture initially expired its 200ms budget
before observing a request. Reverting the emitter sources to the previous commit
reproduced the same failure. That test now allows a one-second budget while
retaining its exact request-count and socket-cleanup assertions; the separate
zero-budget test is unchanged. This was a regression diagnostic, not a paired
performance measurement.

Agent components were reused from the existing rebuilt set; no Agent, WIT or
production host source changed. Dedicated component-host tests, authenticated
server E2E, browser visual review, Linux/soak and paired performance measurements
were not rerun for this stage. E128 and the remaining release gates stay open.

## Deterministic parallel deadline races · 2026-09-08

Four new test groups drive the actual emitted `WindowWait` with per-Agent timers,
ready slots and enclosing alarms. They cover all notification orders for a ready
completion versus its own timer and peer, all orders for enclosing/own/peer
readiness, a normal return during selected timeout cleanup, and root/parent
priority at their existing observation boundaries. The 21 controlled combinations
assert canonical error/result values, retained handles for scheduler assembly,
set membership, alarm ownership and balanced scope deferral. AUDIT-21 maps the
exact tests and limits.

The shared event fixture now supplies both old and new tests, removes queued
notifications when a waitable detaches/drops, and can simulate a callee writing a
late success into its real result slot during cancellation. Removing the emitter's
timeout-result write in a temporary negative control causes the new late-success
test to fail on the result tag. Production code was restored before verification;
this stage changes no emitted code, Agent artifact or host interface.

The retry inventory confirms that production Split and branch eligibility still
falls back to sequential execution for retrying Agent bodies (and Split-level
retries). Preserve that contract during cancellation qualification. The dormant
concurrent-backoff lowering is not evidence of a supported concurrent retry path.

Validation: all 602 default-feature compiler library tests passed (0.81s). The
21 shared-helper tests passed with `direct-wasm-integration-tests` enabled
(0.17s), including all four new race groups. Feature-gated all-target Clippy,
formatting and diff checks passed. The negative-control run failed on the expected
success/error tag assertion, not a compile or fixture setup error. Full composed
workflow/component suites, component builds, database/server E2E and measurements
were not repeated for this test-only change; their earlier results are unchanged
historical evidence. The full feature-gated library now contains 675 tests, but
this stage ran only its 21 affected helper tests.

```sh
cargo test -p runtara-workflows --lib
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib cooperative_wait::tests
cargo clippy -p runtara-workflows --features direct-wasm-integration-tests --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

E128 retirement, real composed race qualification, replay/suspension and CPU
cooperation gaps, obsolete implementation cleanup, final paired measurements,
Linux capacity/soak, authenticated multi-owner server E2E and upstream integration
remain open. No G1–G10 gate is declared complete by this deterministic test stage.

## Retire dormant concurrent Split retries · 2026-09-08

Production eligibility excludes retrying Agent bodies and Split-level retry
policies from parallel windows. The constant-false `concurrent_backoff` branch
still contained a second classify/backoff/reinvoke implementation. It is now
removed, along with unused retry envelope/copy helpers, obsolete slot-state
constants and assembly indirection. Active parallel calls retain their layout,
checkpoint identities and alarm behavior; retrying graphs retain sequential
parking/replay. Host interfaces, migrations and existing artifact execution are
unchanged. AUDIT-22 maps the regression evidence.

A temporary compiler capture on `f107b2fd` and the edited source generated the
same 32 input combinations (durability, Agent/Split retries, parallelism and
scope timeout). All uncomposed output `.wasm` files are byte-identical. The
[comparison record](research/cooperative-retry-retirement-comparison.json) preserves
input dimensions, source fingerprints and output hashes. The temporary capture
test was removed afterward; its source and raw outputs remain under
`/private/tmp/retry-retirement-capture.rs`, `/private/tmp/retry-retirement-before`
and `/private/tmp/retry-retirement-after`. No generated component or `.wasm` artifacts are committed,
and this comparison does not measure runtime latency or memory.

Validation with the pinned toolchain and isolated native/component directories:

- All 602 default-feature compiler library tests passed (0.84s).
- The feature-gated workflow execution suite's `parallel` selection passed
  41 tests (83.75s), including HTTP overlap, cancellation, pause/replay and the
  sequential retry fallback.
- Its `retry` selection passed 91 tests (255.09s), including nested recovery,
  rate-limit budgets, cancellation during backoff, parked replay and inherited
  budget preservation. These selections overlap; their counts are not a unique
  combined-test total.
- Feature-gated all-target Clippy, formatting and diff checks passed. The 32-case
  artifact comparison passed before removing the temporary capture harness.

```sh
cargo test -p runtara-workflows --lib
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute parallel -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute retry -- --test-threads=1
cargo clippy -p runtara-workflows --features direct-wasm-integration-tests --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The complete 395-test execution run, component build/cancellation suite,
database/server E2E, Linux capacity/soak and paired runtime measurements were not
repeated for this removal of unreachable emitter code. Prior results remain
historical evidence. E128, the broader compatibility/obsolete-task inventory and
remaining G1–G10 release qualification stay open.

## Authenticated server E2E and recovery-write races (2026-09-08)

The isolated Python E2E added in `06aed70a` drives the native server's real
authenticated workflow creation/composition/execution and Stop API. Owning-server
header/body cancellation passes, including authentication/role/tenant rejection,
acknowledgement, terminal state, registry retirement, no retries or continuations,
and duplicate Stop. Starting a second server exposes incorrect startup recovery
of a still-live peer execution. The retained database shows the original launch
suspended, a replacement cancelled before start, one recovery attempt and no
Cancel signal for that instance. Thus the original HTTP socket surviving a
successful Stop response is explained by cancellation of the replacement, not by
evidence that guest cooperative cancellation failed. AUDIT-23 records the runs.

The E2E now checks the physical registry generation, lifecycle status, launch
count, recovery attempts and pending commands before and after peer startup,
before issuing Stop. Owner detection is still missing. Existing process-local
grace arming also does not establish remote-owner grace delivery; this remains a
separate follow-up once startup no longer replaces live work.

While tracing recovery, a real PostgreSQL negative control established a second
bug: a stale recovery UPDATE resurrected a `cancelled` row as `suspended` and
replaced its termination reason, wake and counters. Recovery now conditions that
UPDATE on `running`. It reports applied/unchanged outcomes and propagates write
errors, instead of treating an unapplied/failed write as a terminal failure.
Startup retains tracking after unchanged/error outcomes and uses the existing
exact launch-plus-handle cleanup guard after successful decisions. The heartbeat
caller reports recovery errors without claiming they were applied. These changes
do not establish that an owner is dead and cannot distinguish a replacement that
is also `running`; do not read them as resolving the peer-startup failure.

Three database tests cover all non-running statuses, preservation of accepted
lifecycle/result/wake fields, normal running recovery with either policy,
duplicate recovery and database write failure. The registry regression separately
checks replacements that change only the physical handle, only the durable
launch, or both; stale cleanup cannot remove any of them. No emitted WASM, WIT,
Agent component or migration changes in this stage.

Validation uses the pinned toolchain, isolated native/component targets and fresh
`recovery_guard` / `recovery_guard_unit` fixture databases in the retained owned
PostgreSQL container. The initial negative control failed as expected. All eight
focused recovery tests then passed. The broader run passed 243 environment unit
tests (25.00s), 45 handler tests (3.37s) and 21 heartbeat tests (4.06s). A further
22 runtime tests passed after consolidating registry cleanup through the exact
handle guard. These overlapping selections must not be summed as unique tests.
All 12 registry tests passed (1.07s), including the new replacement matrix.
Feature-gated environment/server all-target Clippy passed (113.75s), as did the
native server rebuild (41.08s), formatting, Python syntax and diff checks.

The rebuilt-server E2E repeats the owning-server passes at 0.962s and 0.971s,
then fails before Stop because peer startup changes `running` to `suspended`.
The peer/body case is not reached. These fixture timings are not paired
performance measurements, and the failing E2E remains a release gap. Its retained
logs are under `/var/folders/qf/62s607_11p3bw80y3v821zl80000gn/T/runtara-cooperative-api-82m95b05`;
the command log is `/private/tmp/cooperative-peer-startup-guard.log`.

```sh
cargo test -p runtara-environment --features db-integration-tests --lib recovery:: -- --test-threads=1
cargo test -p runtara-environment --features db-integration-tests --lib --test heartbeat_monitor_test --test handlers_test -- --test-threads=1
cargo test -p runtara-environment --features db-integration-tests --lib runtime::tests -- --test-threads=1
cargo test -p runtara-environment --features db-integration-tests --test container_registry_test -- --test-threads=1
cargo clippy -p runtara-environment -p runtara-server --features runtara-environment/scoped-workflow-integration-tests,runtara-server/db-integration-tests --all-targets -- -D warnings
cargo build -p runtara-server --bin runtara-server
python3 -u e2e/test_cooperative_cancellation.py --server /absolute/path/to/runtara-server --components /absolute/path/to/wasm32-wasip2/release
```

Final component builds, full workflow execution tests, paired performance
measurements and Linux soak are not rerun for this host persistence correction.
The unresolved multi-owner lifecycle cases, E128, compatibility work, upstream
integration and remaining G1–G10 qualification remain required.

## Physical runner identity before owner/grace routing (2026-09-08)

Tracing durable owner handoff exposed another prerequisite: `launch_id` can be
reused after pre-start recovery, but the embedded runner keyed active tasks and
occupancy by that ID and derived its physical handle from it. A real WASM
regression retired one spinning run, installed another with the same durable
launch ID and exercised the old handle. Before the fix, the old handle looked
live, its grace request was accepted and the replacement was stopped. The
negative-control log is `/private/tmp/cooperative-physical-handle-before2.log`.

The existing maps now use an opaque UUID generated per accepted physical
handoff. Liveness, waiting, emergency Stop/grace and completion cleanup resolve
that handle. Durable queue IDs and preparation attempts retain their meaning;
no guest, graph, WIT, schema or timer/task-count change is introduced. The mock
runner uses the same physical identity for control/results and does not confuse
retired results with a later execution. The runner/registry field documentation
now distinguishes durable launch identity from physical handle identity.

The real reuse regression passes. A second real-runner test installs overlapping
closed gates with the same durable launch ID, stops the first before guest
execution, and verifies that its retirement leaves the second handle and
occupancy age intact. The mock test covers stale control and separate results.
An older timeout-handler fixture fabricated a handle and relied on the runner
ignoring it; it now uses the actual returned handle. That fixture initially
failed the stricter contract, then passed after correction.

Verification for this stage:

- All 10 embedded-runner integration tests passed (4.94s), including both new
  physical identity regressions and infinite invocation/initializer grace abort.
- With the production runner fix, all 243 environment unit tests, four composed
  cooperative Stop tests (15.73s), 45 handler tests, nine launch-queue tests and
  seven existing packaged-runner compatibility tests (15.69s) passed.
- After the mock alignment, all 244 environment unit tests passed (25.13s).
  After correcting the fabricated-handle fixture, all 45 handler tests (2.59s)
  and nine launch-queue tests (2.41s) passed again. Counts overlap across stages;
  these are not one summed set of distinct tests.
- Feature-gated environment/server all-target Clippy passed (25.64s), as did
  formatting and diff whitespace checks.
- The native server rebuild passed (53.65s). The authenticated E2E again passes
  owning-server header/body cancellation (0.968s / 0.984s), then fails before
  Stop because peer startup changes `running` to `suspended`. The peer/body case
  is not reached. This is an unresolved failing E2E, not a passing suite.
  Its command log is `/private/tmp/cooperative-physical-handle-server.log`; the
  retained fixture directory is
  `/var/folders/qf/62s607_11p3bw80y3v821zl80000gn/T/runtara-cooperative-api-ak7rgrzq`.
  These individual fixture durations are not controlled latency measurements.

Commands use the pinned toolchain, isolated native/component directories and
the owned `recovery_guard` fixture databases. The final mock/handler verification
uses the existing `RUNTARA_MAX_CONCURRENT_RUNS=4` setting; the overlapping-gate
fixture requires at least two run slots.

```sh
cargo test -p runtara-environment --features db-integration-tests --test embedded_runner_test -- --test-threads=1
cargo test -p runtara-environment --features scoped-workflow-integration-tests --lib --test handlers_test --test launch_queue_test --test cooperative_stop_test --test scoped_runner_test -- --test-threads=1
cargo test -p runtara-environment --features db-integration-tests --lib --test handlers_test --test launch_queue_test -- --test-threads=1
cargo test -p runtara-environment --features db-integration-tests --test handlers_test --test launch_queue_test -- --test-threads=1
cargo clippy -p runtara-environment -p runtara-server --features runtara-environment/scoped-workflow-integration-tests,runtara-server/db-integration-tests --all-targets -- -D warnings
```

The rebuilt-server check uses:

```sh
cargo build -p runtara-server --bin runtara-server
python3 -u e2e/test_cooperative_cancellation.py --server /absolute/path/to/runtara-server --components /absolute/path/to/wasm32-wasip2/release
```

No full component rebuild, full compiler execution suite, paired size/timing
measurement or Linux soak is claimed for this native handle change. Physical
identity is required for ownership fencing but does not establish owner liveness,
lease renewal or remote grace delivery. The peer-startup failure and all remaining
G1–G10 gates remain open. AUDIT-25 maps the contracts to the tests.


### Remote whole-execution grace delivery · 2026-09-08

The preceding `b932f1c6` snapshot retained existing launch ownership leases and
recorded AUDIT-26. Rebuilding that source passed (42.24s), and authenticated E2E
proved peer startup now preserves the live run. Owner/header and owner/body
passed (0.998s / 1.026s); peer/header then returned HTTP 500 from the local-only
grace path. The failing run is `/private/tmp/cooperative-owner-server-e2e.log`,
with retained isolated fixture `runtara-cooperative-api-n5hrcxkb` beneath the
system temporary directory. This isolated remote grace from startup recovery.

The new forward migration adds requested/armed deadline timestamps to the
existing physical registry. A peer persists an absolute deadline derived from
the request's remaining monotonic budget and the database clock. The existing
launch dispatcher delivers a bounded batch for its own live claims before queue
work, including during drain. It arms the native physical handle before recording
acknowledgement. A peer waits up to five seconds for that acknowledgement or an
accepted terminal outcome; lack of confirmation remains an error. This wait
never extends the stored grace. Replacement registrations clear control state;
same-handle upserts and duplicate Stop preserve the earliest deadline.

The change adds no guest ABI, per-step task manager, worker, graph orchestration,
component binary or product flag. Whole-execution emergency abort remains native;
cooperative Cancel consumption, cleanup and graph decisions remain in WASM.
Database-clock jumps, late delivery, partitions, owner disappearance/recovery,
small-pool pressure and load still need qualification. The renewal writes and
indexed pending-deadline scan belong in the outstanding G10 comparison.

Verification on this change:

| Suite | Result |
| --- | --- |
| Handler and launch queue | 45 + 15 pass (2.92s / 9.24s) |
| Environment library, physical registry and composed Stop | 244 + 12 + 4 pass (24.06s / 0.79s / 15.54s) |
| Embedded WASM and heartbeat after fixture correction | 13 + 21 pass (7.21s / 4.22s) |
| Feature-gated environment/server all-target Clippy | Pass, 24.15s |
| Native server rebuild | Pass, 35.60s |
| Authenticated owner/peer HTTP E2E | All four cases pass; extended repeat also passes sustained lease renewal and unrelated peer shutdown |

These are 354 distinct tests across the selected Rust suites, not one complete
workspace run. The initial embedded run had 12 passes and a failure caused by the
new fixture reusing a tenant/image name; the fixture now uses its unique image ID
as the name, and the entire 13-test suite passed again. The subsequent heartbeat
suite also passed. Python syntax and diff whitespace checks passed. The commit
uses the normal formatting/workspace Clippy pre-commit hook without bypass.

The E2E's original four-case pass observed owner headers/body at 1.017s / 0.849s
and peer headers/body at 0.545s / 0.587s. The extended repeat holds pending HTTP
for 32 seconds, proves the owner's lease expiry advances, starts and shuts down
an unrelated peer, and checks the original physical run and pending-signal state
are preserved before Stop. All extended cases pass at 0.980s / 1.007s and
1.119s / 0.640s respectively. These timings include fixture verification and
are not controlled cancellation benchmarks. The 90-second HTTP timeout in the
fixture avoids its default 30-second timeout masking the ownership test.

Logs:

- `/private/tmp/cooperative-remote-grace-tests.log`
- `/private/tmp/cooperative-remote-grace-broad-tests.log` (includes the fixture failure)
- `/private/tmp/cooperative-remote-grace-native-tests.log`
- `/private/tmp/cooperative-remote-grace-clippy.log`
- `/private/tmp/cooperative-remote-grace-server-e2e.log`
- `/private/tmp/cooperative-remote-grace-renewal-e2e.log`

The extended E2E retains its own database/log fixture at
`/var/folders/qf/62s607_11p3bw80y3v821zl80000gn/T/runtara-cooperative-api-lmx6jkrf`;
its own servers and containers were stopped by the harness. Tests use fresh
isolated credentials generated in memory, never existing connection settings.
Rust tests use the owned isolated `recovery_guard` database, pinned toolchain,
existing matching Agent components and native target, with
`RUNTARA_MAX_CONCURRENT_RUNS=4` for overlapping physical handoffs.

```sh
cargo test -p runtara-environment --features scoped-workflow-integration-tests --test launch_queue_test --test handlers_test -- --test-threads=1
cargo test -p runtara-environment --features scoped-workflow-integration-tests --lib --test embedded_runner_test --test cooperative_stop_test --test heartbeat_monitor_test --test container_registry_test -- --test-threads=1
cargo test -p runtara-environment --features scoped-workflow-integration-tests --test embedded_runner_test --test heartbeat_monitor_test -- --test-threads=1
cargo clippy -p runtara-environment -p runtara-server --features runtara-environment/scoped-workflow-integration-tests,runtara-server/db-integration-tests --all-targets -- -D warnings
cargo build -p runtara-server --bin runtara-server
python3 -u e2e/test_cooperative_cancellation.py --server /absolute/path/to/runtara-server --components /absolute/path/to/wasm32-wasip2/release
```

The full component/compiler matrices, baseline/candidate size and latency
measurements and Linux soak were not rerun for this native lifecycle change.
No G1–G10 completion is claimed. E128 remains, and the audit's consolidated
remaining-work table still governs final qualification and upstream/PR work.


### Guest deadlines for Agent tools · 2026-09-08

Public-timeout review found an unqualified invocation path: the AI-loop Agent
tool arm injected `timeout_ms` through `ai-tool-args-with-timeout`, while
`emit_agent_invoke` selected an own budget only for ordinary `Step` sites.
Consequently, simply dropping E128 would not provide the same deadline contract
for an Agent invoked as a model tool. AUDIT-28 records the finding and contract.

The Agent tool plan now carries its original definition step and effective
manifest durability. `agent_tool_deadline` reuses scoped source construction,
Agent budget arithmetic and the common checkpoint helpers. It scopes each call
by the existing AI step, tool label and replay-stable call counter. The shared
Agent invoke/connection-preparation path also selects deadlines for `AiTool`.
A local timeout becomes non-retryable `AGENT_TIMEOUT` model feedback after
standard subtask cleanup. Root cancellation or enclosing expiry escapes without
new feedback or model work. Capability inputs keep their authored values.

Durable completed results bypass their old budgets on replay. Pending budgets
persist through pause and include parked time; the next model-selected call
gets its own budget. The new result checkpoint also covers the gap between tool
completion and the enclosing AI turn snapshot. No new authored syntax, product
flag, task resource, host bookkeeping, guest import or runtime component source
is introduced. The old timeout argument-merge compiler call/index fields are
removed, while the stdlib export remains available for old artifacts. The cache
tag advances to `cooperative-waits=shared-v18`; parked/registered artifacts are
not rewritten.

Verification:

- Seven new composed Agent-tool tests pass (7.95s), covering durable/non-durable
  zero budgets, headers/body cleanup, fresh subsequent calls, maximum unsigned
  budgets, provider errors, root/enclosing selection, completed replay, expired
  pending replay and malformed budget checkpoints. HTTP envelopes retain their
  explicitly authored 90,000 ms I/O timeout when the workflow budget is 400 ms.
- A negative control removed `AiTool` from own-deadline selection. The zero-budget
  regression failed with `unexpected child request 0` (1.15s). The original
  source was restored before all subsequent tests.
- The complete feature-gated compiler library suite passed **682 tests** in
  412.38s. This includes the seven new tests, so counts must not be summed.
- The public `direct_wasm_execute` AI selection passed **19 tests** in 14.00s
  (379 tests filtered out). It covers existing AI workflows and capability
  `turnTimeout`; it does not enable or prove authored Agent/Embed timeout syntax.
- Feature-gated workflows all-target Clippy with `-D warnings` passed (4.15s).
  Formatting and diff whitespace checks passed. The commit uses the normal
  formatting/workspace all-target Clippy hook without bypass.
- The first expanded test compilation found another shared HTTP fixture
  constructor that needed the new request-capture field. After updating that
  constructor, the expanded and full suites above passed.

Commands use the pinned toolchain, `RUSTC_WRAPPER=`, `SQLX_OFFLINE=true`,
`CARGO_BUILD_JOBS=4`, the existing isolated native target and previously built
matching Agent/shared components:

```sh
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib agent_tool:: -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute ai_agent -- --test-threads=1
cargo clippy -p runtara-workflows --features direct-wasm-integration-tests --all-targets -- -D warnings
```

Logs are `/private/tmp/cooperative-agent-tool-deadline-expanded.log`,
`/private/tmp/cooperative-agent-tool-negative.log`,
`/private/tmp/cooperative-agent-tool-full-lib.log`,
`/private/tmp/cooperative-agent-tool-public-ai-tests.log` and
`/private/tmp/cooperative-agent-tool-clippy.log`.

E128 and direct-support rejection remain. These authored-budget tests still use
the existing private emitter harness; no success is claimed for public timeout
validation/compilation. The review additionally found that AI memory and
synthetic MCP provider construction drops the referenced Agent timeout and
sets generated invocation metadata to `None`. Carrying and enforcing those
budgets is the next public-enablement work. Then move the deadline corpus onto
public compilation and verify component import inference and export modes.
The remaining G1–G10 work, full public execution matrix, authenticated E2E,
component rebuilds, paired size/latency report and Linux soak were not repeated
or completed by this compiler stage.
