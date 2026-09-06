# Cooperative cancellation implementation record

Status: sequential and parallel emitted root cancellation, 2026-09-06. Governing contract:
[cooperative cancellation plan](selective-isolation-plan.md). Update the existing
implementation directly; no new product feature flags or alternate backend.
Nested workflow-agent cancellation, timeouts and the remaining plan gates are
still incomplete.

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
measurements remain linked from the plan; fresh paired measurements are pending.

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
| `async_typing_with_synchronous_bindings_does_not_acknowledge_cancellation` | Same parent/control path with the existing agent ABI shape remains unresolved until the test watchdog. A trap or invalid fixture does not count as the expected result. |

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
