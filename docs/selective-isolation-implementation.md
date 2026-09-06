# Selective isolation implementation record

Objective: implement [the full plan](selective-isolation-plan.md), preserving the
DSL and existing support, with small tested commits and final local-server E2E.
This record tracks implementation; the earlier research is not a completion claim.

## Starting state

- Fetched `origin/main` on 2026-09-06 and created `feat/selective-isolation`
  directly at `024c4c5c131314f98c67c58f3ac1be1b7a1e921e`.
- Upstream already contains audit PR #227 and environment ownership PR #228.
- Preserved research as `265cfe11`, benchmark tooling as `612283d0`, and the plan
  and measurements as `0867b13b`. Repository commit hooks passed.
- Fresh upstream-based baseline: 840 workflow tests passed; existing doctest and
  the manual benchmark ignored. Component host: 59 passed, manual benchmark ignored.

## Implemented and verified

### Process-local isolated task ownership

Committed as `c864d953`.

`runtara-component-host::isolated_tasks` provides owner-scoped non-reused handles,
fail-fast admission, aggregate retained-result accounting, cancellation/join/release,
and root shutdown. Completion versus cancellation has one locked publication
decision; a cancelled ready result cannot win merely because its future polls first.
The worker's owned future is dropped before the result becomes visible. Native
worker panic produces a terminal result rather than leaving join pending forever.

The Store runner must install the supplied token in its epoch callback before
instantiation. Actual guest-loop and infinite-initializer tests demonstrate this
contract, preserving a sibling and observing Store destruction before join. Pending
execution cleanup, pre-start cancel, duplicate/late commands, owner checks, stale
handles, capacity release, retained-result readers and reserved Vec capacity are
also tested. A root runner must explicitly await shutdown; Drop only requests
last-resort cancellation.

Verification: 13 new tests passed; complete component-host suite with both
integration/PoC features: **72 passed**, one manual benchmark ignored. Focused
all-target Clippy with both features and `-D warnings` passed.

This is the live ownership layer. It does not yet expose a WIT import, redirect
DSL Agent calls, implement persistent cancellation, or change a production default.

### Self-contained package contract

The optional `runtara-workflow-wit/isolation-package` module defines the shared
compiler/host catalog format. It appends one versioned custom section to a root
component, stores each child once by SHA-256, and binds package-local names to
those bytes. Parsing borrows verified child slices; it rejects overlapping bodies,
unknown versions, invalid references, duplicate bindings, trailing/unindexed data,
nested catalogs and native-code inputs. Bounds apply before decoding the index.
Existing components without a catalog remain unmodified.

Verification: 10 package tests plus the five existing WIT contract tests passed.
A component-host integration test packages 100 bindings to one child, validates
the package with Wasmtime and executes both its root and resolved child. Focused
all-target Clippy passed. The guest-only WIT dependency stays light: package codec
dependencies are behind the host/compiler feature.

This establishes packaging and validation; it does not yet select a compiler
backend or expose the catalog to running workflows.

### Native package preparation

The precompile worker now validates the raw package and precompiles the root and
every unique child into one bounded native response. The existing private-worker
nonce, full source digest, engine fingerprint and serialized digest protect the
whole response. The new trusted package decoder validates member framing and
bindings before loading the prepared components; native bytes still require the
same trusted provenance as legacy responses. Legacy components keep their existing
native encoding. The root-only decoder explicitly rejects packages so a caller
cannot silently discard isolated dependencies.

Verification: three new native codec tests cover legacy compatibility, deduplicated
roundtrip with 100 bindings, truncation/trailing bytes and corrupted child rejection.
The real package integration test now runs both root and child after worker
precompilation, rejects a wrong nonce and checks the root-only decoder rejection.
The complete component-host suite with integration/PoC features passed **76 tests**,
with one manual benchmark ignored; focused all-target Clippy passed.

Native compilation stays in the worker, never in `start`. The prepared catalog
transport is described below; child execution imports remain to be connected.

### Guarded capability Store execution

`WorkflowExecutor::execute_isolated_capability` now shares the lifecycle runner's
Store setup, WASI sandbox, resource limiter, HTTP deadline, epoch callback and
pending-host-call watchdog. The task token and root cancellation flag remain
independent inputs: either can interrupt the child. Guards are installed before
instantiation, and a root already cancelled at entry never runs an initializer.
The Agent boundary returns existing structured errors and bytes; graph retry and
recovery policy are still the caller's responsibility.

Seven new fixture tests exercise fresh Store state and large byte roundtrips,
structured retry metadata, cancellation during execution/initialization, sibling
survival, pending host-call destruction, pre-start cancellation, deadlines, memory
limits and traps. A real-component integration test invokes `random-double` while
an actual HTTP Agent is blocked against a local endpoint, then cancels the HTTP
child while preserving the random result. These focused tests passed. The complete component-host suite with both
integration/PoC features passed **84 tests**, with one manual benchmark ignored;
focused all-target Clippy passed. After this shared-runner change, the full
workflow suite passed **840 tests** (565 library, 221 composed execution and 54
other integration tests); the manual benchmark and existing doctest were ignored.

This runner is not yet wired to generated workflow imports. The compiler backend,
child runtime authority adapter, aggregate resource accounting and durable attempt
fencing remain required; the execution method alone does not provide them.

### Prepared catalog lifetime through launch and cache

The environment now decodes the complete trusted worker package and links its
unique children alongside the root. `PreparedWorkflow` owns an immutable catalog;
queued tokens and opt-in cache hits share its prepared definitions. Bindings can
resolve only package-local child ids and the catalog's declared interface. Linking
rejects missing/duplicate bindings, missing interfaces and unreferenced members
without constructing a Store. The initial implementation checked the full entry
ABI when obtaining the typed function; the preparation-time check described below
now rejects incompatible signatures before initialization as well.

Two catalog fixture tests check deduplication, token cloning, invalid references
and the absence of initializer execution. The real-component cache test runs in
separate processes with caching both disabled and enabled. It precompiles a raw
package through the worker, verifies its digest on cache reuse, deletes the source,
drops the original executor/cache and executes the retained root and random-double
child using a new executor sharing the engine.

Verification: **87 component-host tests passed**, with one manual benchmark
ignored. All **12 environment embedded-runner unit tests passed**. All-target
Clippy passed for component-host with both integration/PoC features and environment
with its database-test feature. Database integration tests and a server launched
through its actual worker subprocess have not yet been run for this change; they
remain part of the final local-server gate.

This connects preparation and ownership, not the generated execution imports or
root-to-child invocation context. No production backend default has changed.

### Guest execution resource ABI and host imports

The canonical `runtara:workflow-execution/tasks@0.1.0` interface now defines
owned task resources, start/cancel, asynchronous join/release, relative invocation
context and typed completion/error/suspension/cancellation/timeout/trap outcomes.
The WIT parser validates its resource and async shapes; all **16 WIT tests passed**.

`execution_host` provides the generic linker adapter over `IsolatedTasks`. An
embedding-owned launcher resolves prepared bindings and validates relative context;
guests cannot install that launcher. Task handles belong to the parent execution
context and are checked again at the host table boundary. Joining preserves error
retry metadata and full wake sets. Explicit release cancels/reaps work while the
resource remains owned; resource-drop cancels/reaps and returns its handle quota.
Store destruction requests cancellation for handles the guest failed to drop;
the embedding must still await context shutdown before releasing the parent.

A separate handle quota closes a gap between task release and resource-drop:
released-but-undropped resources cannot grow the host table indefinitely. Tests
confirm capacity is returned only when the handle itself is destroyed.

Nine focused tests passed, including parent-WASM control flow that starts a pending
child, joins a sibling, cancels/joins the pending child and executes its own recovery.
A real-component variant runs the actual HTTP and random-double agents and keeps
the HTTP endpoint hanging until parent WASM cancels it. Other cases cover resource
destruction, parent traps, wrong-parent handles, unavailable/closed/full contexts,
invalid binding/context errors, owned bytes, retry metadata and multi-entry wake sets.
The tests also establish the required canonical borrow cleanup for forwarding shims.
The complete component-host suite with both integration/PoC features passed
**96 tests**, with one manual benchmark ignored; focused all-target Clippy passed
for both component-host and the WIT crate.

This chunk established the execution ABI and its host adapter. The production
root Store wiring is described below; a real scoped launcher/runtime adapter
is still required. The descendant
cleanup primitive described below is available for that wiring. Aggregate input/transport/guest
memory accounting is also still required; the handle quota does not replace it.
No DSL backend or production linker default has been enabled by this chunk.

### Descendant cleanup before publication

`IsolatedTasks::spawn_scoped` keeps a separate cleanup future in the supervisor,
not inside the cancellable execution future. The supervisor first joins/drops
execution, then awaits descendant teardown, then arbitrates and publishes the
terminal result. This ordering also applies to pre-start cancellation and worker
panic. A cleanup failure or panic closes the result channel with `WorkerLost`;
it never fabricates a successful, cancelled or recoverable guest-trap outcome
when teardown cannot be confirmed. The production coordinator must treat this
as a host failure and fence/stop the root, not run ordinary step recovery.

`PreparedInvocation` carries the execution factory and optional cleanup into the
resource adapter. `ExecutionContext::into_cleanup` transfers an owned child scope
to its parent's supervisor. Leaves keep the existing two-task execution path;
scopes with descendants use an additional supervised cleanup task to observe
cleanup panics. Its overhead belongs in the candidate comparison measurements.

Verification: five new registry tests plus one resource-ABI test passed. They
prove publication waits at an explicit cleanup barrier, cleanup runs after
pre-start cancellation and parent panic, and failed cleanup reaches the ABI as a
host error. Actual non-cooperative WASM loops and infinite initializers run as
grandchildren; cancelling their parent destroys their Stores before the parent's
join completes, while preserving a root sibling. The full component-host suite
passed **102 tests**, with one manual benchmark ignored; focused all-target
Clippy passed.

The production launcher must still construct and register these child scopes;
no generated workflow is using the new execution ABI yet. This primitive does
not implement durable attempt fencing or native blocking-work interruption.

### Production Store context and supervised root execution

The production `WorkflowState` now implements the execution-resource view and
`WorkflowExecutor` links the versioned task interface. Existing entry points
retain an absent launcher. `execute_invoke_with_context` enables an externally
owned context for one root run; `execute_isolated_workflow` and
`execute_isolated_capability_with_context` accept separate child contexts and
preserve both root and task cancellation guards.

The context-enabled root runs under an owned supervisor. The supervisor joins
the root worker, then awaits descendant shutdown, then returns the outcome and
elapsed duration. A successful or suspended guest result cannot bypass cleanup.
Worker panic or cleanup failure becomes a root host failure. Dropping the caller
future requests interruption through an epoch flag and an asynchronous wake;
the supervisor continues cleanup even when the root was waiting at its durable
start-confirmation gate. Child entry points deliberately leave descendant cleanup
with their owning task supervisor, outside the cancellable execution future.

Nine new tests execute canonical parent WASM through the production linker and
runner. They cover guest-driven cancellation and recovery, unreleased children
on successful return/trap, cleanup barriers and failure, CPU timeout, root cancel
while joining, caller abandonment, closed/panicked/abandoned start gates,
pre-start cancellation, legacy unavailable context, and isolated lifecycle entry
with both task and root cancellation. The launcher in these new tests uses
controlled futures; existing real HTTP/utils component tests also passed in the
full **111-test component-host suite**. Focused all-target Clippy passed. The
workflow regression suite also passed **840 tests**, including all 221 emitted
WASM executions; the manual performance benchmark and existing doctest were
ignored as expected.

The server/environment launch path still needs to construct the real scoped
launcher/runtime adapter and select this context-enabled entry point for isolated
artifacts. Its admission permit and durable launch ownership must survive until
tree teardown is confirmed, including caller abandonment; supervision inside
component-host alone does not transfer ownership of those external resources.
No compiler backend or runtime default is enabled by this change.

### Cleanup failure remains visible after repeated shutdown

A new regression test reproduced a registry bug: after the first failed shutdown
cleared its task map, a concurrent or later shutdown returned success. The registry
now retains the cleanup-failure state after releasing task metadata. Every later
shutdown reports `WorkerLost`, so a root supervisor cannot mistake a previous
unconfirmed teardown for success. Admission remains closed and retained result
buffers are released.

The test failed before the fix and passed after it. All **19 task-registry tests**
passed, followed by the full **112-test component-host suite** and focused
all-target Clippy with both integration/PoC features. This is a live ownership
fix; durable root fencing remains a separate required integration.

### Prepared package invocation launcher

`PreparedInvocationLauncher` now resolves Agent and child-workflow invocations
from the catalog owned by the verified preparation token. It rejects absent
bindings, mismatched entry kinds and catalogs from a different engine before
constructing an invocation scope. The selected interface comes from the package,
not guest input; lifecycle bindings select their exact exported version even
when a component offers both lifecycle versions.

The embedding must supply an `InvocationScopeFactory` bound to parent/root/tenant
authority. There is deliberately no default that copies the root RuntimeHost.
The factory validates relative invocation metadata and constructs child scope;
its spec factory runs only inside the admitted task and receives that task's
cancellation token. Any descendant context is retained separately by the task
supervisor for cleanup after execution. The production persistence-backed scope
factory and child runtime adapter still need implementation.

Six new tests passed: binding/entry/scope rejection before initialization,
repeated fresh component state with owned large byte payloads, pre-start cancel
without spec/Store construction, child-scope cleanup, parent-WASM cancellation
through the real launcher, inherited root cancellation, engine mismatch, exact
lifecycle version and completion/error/suspension outcomes. The existing raw
package/worker/cache integration test now invokes the actual random-double agent
through this launcher after deleting its source and dropping the preparation
owner; cache-enabled and cache-disabled subprocess cases both passed.

The full component-host suite passed **118 tests** with one manual benchmark
ignored; all-target Clippy passed with both integration/PoC features. The workflow
regression suite passed **840 tests**, including all 221 emitted WASM executions,
with its manual benchmark and existing doctest ignored. Generated
DSL workflows still use the legacy backend, and server launch selection remains
unchanged until scoped runtime authority and the compiler are wired.

### Reject incompatible child ABIs before initialization

Preparation now validates the complete invocation signature using Wasmtime's
prepared type metadata. Capability entries require a string capability argument
and byte-list input/result; lifecycle entries require byte-list input and the
full completed/suspended outcome. Error fields, optional payloads, signal waits,
and wake variants must match the canonical host/WIT contract, including order.
Both sync and async function shapes remain supported. Wasmtime still checks the
typed function at invocation as a second check.

The regression test first demonstrated that preparation accepted a component
with an incompatible input element type. After the fix, nine valid-component
fixtures with incompatible argument counts, return forms, input/output element
types, error fields/payloads, outcome ordering and wake deadlines are rejected
without executing an initializer. Positive coverage includes actual built utils
through the worker/package/cache path and both lifecycle versions. The full
component-host suite passed **119 tests** with one manual benchmark ignored;
focused all-target Clippy passed with both integration/PoC features.

This validation applies to the new isolated package bindings. Legacy artifacts
retain their existing execution path and accepted DSL support.

### First emitted isolated Agent execution path

The explicit `compose_direct_workflow_with_isolated_agents` API replaces selected
Agent dependencies with small guest components that keep the existing Agent
`invoke` ABI and perform task start/join/release in WASM. Existing parent lowering
continues to own mapping, retry/error handling, edges and parallel pools. The
complete distributable artifact embeds each selected component once in the raw
package catalog; result size/checksum metadata covers that complete artifact.

Selection pins caller-reviewed, reset-safe component bytes by SHA-256. Unknown
selection keys and changed reviewed bytes are rejected. Unselected dependencies
retain their existing component lifetime; an empty selection preserves legacy
artifact bytes and metadata. Optional artifact metadata lists isolated bindings,
legacy Agent IDs, adapter version and the current `live-adapter-call:1` context
contract. The server metadata test fixture accepts this additive field.

This first context identifies a live call by its binding and adapter-local call
ordinal. It is **not** a replay-stable logical step/attempt address; pool instances
have separate ordinals. Host task ownership still distinguishes live resources,
but durable targeted cancellation must wait for compiler-generated logical scope
propagation and the production scope factory. No server/default selector changed.

New execution tests run actual compiled DSL through the worker/prepared-package
path and production child launcher: random-double chains of 1/10/100 calls,
parallel Split, and a 1 MiB Unicode input/output round trip. Counts verify actual
child invocation rather than fallback. Catalog assertions verify deduplication;
small handle limits with repeated calls and retained-result checks verify release.
Large payload and failure results are compared with legacy execution. A mixed
utils/datetime graph verifies that an unselected Agent still executes and appears
in the metadata as using its existing component lifetime.

Direct emitted-adapter tests cover raw output bytes and every error-info field,
including `retry-after-ms = u64::MAX` and Unicode attributes. Cancellation and
timeout become nonretryable errors; child traps, unsupported capability suspension
and host execution errors remain fatal traps. The guest arena grows with checked
32-bit bounds and resets after canonical post-return when calls have completed.

Verification: the full workflow run passed **847 tests** (568 library, 225 emitted
WASM executions and 54 other integration tests); the manual performance benchmark
and existing doctest remained ignored. A subsequent focused run passed all five
emitted-isolation tests, including the added mixed selected/unselected Agent case,
package-limit failure preserving the previous artifact, and metadata round trips.
The three direct adapter ABI tests also passed. Rebuilding all 27 Agent components
and both shared components succeeded; staged WASM hashes were unchanged. All five
emitted-isolation tests passed again against the rebuilt bundle.
The component-host suite passed **119 tests** with its manual benchmark ignored;
a missing worktree build-output link caused one initial fixture-location failure
and was restored before that successful run. Focused all-target workflow Clippy
passed with the integration feature and `-D warnings`.

### Paired measurement harness

A separate ignored `workflow_performance_comparison` test uses the same 11 workload
fixtures as the historical baseline. It selects the actual composed Agent adapter,
uses the production bounded worker package codec with an explicitly cache-disabled
engine, and retains complete raw/gzip/native package sizes, preparation phases,
fresh full executions, instrumented counter checks and checkpoint replay samples.
Embed remains inline in this candidate; Finish-only graphs are explicit controls.

The explicit-engine precompiler shares the default worker's bounded reading,
package encoding, integrity fields and trusted deserialization contract. The
worker entry still creates its default engine. A configuration-mismatch regression
test proves the new entry honors its supplied engine. It does not make synchronous
compilation cancellable; the process worker remains necessary for that boundary.

Run the release harness serially with alternating first backends, for example:

```sh
RUSTC_WRAPPER= RUNTARA_BENCH_FIRST=legacy cargo test --release -p runtara-workflows \
  --features direct-wasm-integration-tests --test direct_wasm_execute \
  workflow_performance_comparison -- --ignored --nocapture > /tmp/isolation-comparison-1.log 2>&1
RUSTC_WRAPPER= RUNTARA_BENCH_FIRST=isolated-agent cargo test --release -p runtara-workflows \
  --features direct-wasm-integration-tests --test direct_wasm_execute \
  workflow_performance_comparison -- --ignored --nocapture > /tmp/isolation-comparison-2.log 2>&1
```

Use `scripts/research/workflow_comparison_report.py` with `--logs`, `--output`,
`--markdown`, `--date`, `--source-revision` and `--machine` to validate and render
paired results. Commit implementation before measured runs; revision, executable
hash, dependency hashes, configuration, workloads, sample summaries and isolation
counts are checked before reports can be combined. Preserve the historical
baseline separately because it used a different preparation timer boundary.

The debug smoke test executes both backends for all 11 workloads; it is correctness
evidence only. Release runs use 100 samples for small cases and 30 for large cases,
with separate instrumentation runs and three compilation samples. These remain
exploratory; they do not provide the required production tail evidence. Direct
Agent/parent-step spans, aggregate memory, server persistence and cancellation
measurements remain pending.

### Measured Agent-only comparison

Committed harness `fe014566` completed three serial release sessions, alternating
legacy/isolated/legacy first. All 11 workloads passed on both backends in every
session, including output checks, real child-count assertions and checkpoint
replay. The [comparison report](research/workflow-performance-comparison.md) and
[raw samples](research/workflow-performance-comparison.json) retain executable,
source, workload and component identities; the report validator accepted all three
sessions together. There were no excluded failed samples.

For default random-double + Finish, full WASM grows from 3,308,198 to 3,314,102 bytes
(+0.18%), gzip grows 0.16%, and complete native worker payload grows 0.61%. Prepared
full-run p50 rises from 0.155–0.162 ms to 0.215–0.227 ms (+33–47%). Ten random calls
rise from 0.474–0.487 ms to 1.041–1.127 ms (+117–131%). These costs require evaluation
against deployment budgets; small file-size growth does not imply small execution
overhead. Cold phases have just three samples per session and visible control
variation, so they are exploratory evidence rather than acceptance thresholds.

Verification: all 22 backend/workload combinations passed the debug smoke test;
10 focused precompile tests (including explicit engine configuration and package
round trips) passed; nine report-validator tests passed. Integration-feature
all-target Clippy and the workspace commit hook passed. The historical baseline
is retained separately because its preparation boundary differs. Direct Agent
and parent-step spans, full aggregate resource accounting and server timings are
still pending; the candidate is not production qualification.

## Remaining required work

- P0: extend explicit differential selection and invocation-count evidence to all
  required constructs and production artifact inspection.
- P1: production invocation-scope factory and environment ownership integration, aggregate
  input/transport/guest resource reservations and root fencing on cleanup failure.
- P2: propagate logical step/attempt scopes through the Agent backend, qualify
  every AI auxiliary invocation and certify package reset/state eligibility.
- P3: recursive Embed extraction and scoped child runtime, suspension/wake sets,
  scopes, deadlines, checkpoint keys and existing reference ABI modes.
- P4: durable attempt transitions, root and targeted command routing, crash/lease
  fencing, resource/tenant ownership and parked invocation handling.
- P5: all compatibility gates, extend the paired Agent measurements to direct
  step spans, aggregate resources and production qualification; full unit and
  integration suites, local server plus isolated persistence E2E.
- P6: controlled opt-in and artifact-compatible rollback; no default enablement
  before all gates above pass.

No local server has been launched yet. The experimental emitted Agent path has
correctness and paired local performance evidence. It does not yet establish
durable targeted cancellation or completion of the full plan.
