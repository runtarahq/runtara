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
without constructing a Store. The guarded invocation still type-checks the full
entry ABI when it obtains the typed function.

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
ignored; all-target Clippy passed with both integration/PoC features. Generated
DSL workflows still use the legacy backend, and server launch selection remains
unchanged until scoped runtime authority and the compiler are wired.

## Remaining required work

- P0: add explicit legacy/isolated differential selection and coverage counters.
- P1: production invocation-scope factory and environment ownership integration, aggregate
  input/transport/guest resource reservations and root fencing on cleanup failure.
- P2: sequential/parallel Agent call backend and every AI auxiliary invocation,
  preserving package state eligibility and existing invocation semantics.
- P3: recursive Embed extraction and scoped child runtime, suspension/wake sets,
  scopes, deadlines, checkpoint keys and existing reference ABI modes.
- P4: durable attempt transitions, root and targeted command routing, crash/lease
  fencing, resource/tenant ownership and parked invocation handling.
- P5: all compatibility gates, actual baseline/candidate measurements, full unit
  and integration suites, local server plus isolated persistence E2E.
- P6: controlled opt-in and artifact-compatible rollback; no default enablement
  before all gates above pass.

No local server has been launched yet. No isolated DSL backend result has been
measured. Do not treat the task-ownership tests as proof of the full implementation.
