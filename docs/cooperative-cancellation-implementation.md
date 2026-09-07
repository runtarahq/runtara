# Cooperative cancellation implementation record

Status: sequential/parallel root cancellation and Agent binding migration, 2026-09-07. Governing contract:
[cooperative cancellation plan](selective-isolation-plan.md). Update the existing
implementation directly; no new product feature flags or alternate backend.
Nested workflow-agent cancellation, timeouts and the remaining plan gates are
still incomplete.

The public Stop path still calls `Runner::stop` immediately and ignores its
accepted grace-period field. The emitted-wait proofs below exercise a delivered
lifecycle Cancel signal; they do not yet qualify cooperative Stop through the
public API. The existing post-run cancellation backstop can also overwrite a
completed outcome. Grace/escalation and completion-race semantics remain P3 work.

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
