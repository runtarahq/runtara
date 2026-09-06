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

### Compiler-owned Agent call identity

The opt-in `compile_direct_workflow_with_scoped_agents` API emits a private
`scoped-capabilities` interface for explicitly selected dependencies. Composition
requires exactly the same reviewed SHA-256 selection; metadata records adapter
version 2 and `logical-agent-call:2`. The ordinary compiler and adapter-v1
composition APIs retain their defaults. Public Agent capabilities still receive
only their capability name and original input bytes.

The parent guest derives the relative path from its existing structured Agent
checkpoint key, including workflow, inherited namespace and loop ancestry. A
fixed-width suffix distinguishes ordinary steps, memory load, LLM turns, Agent
tools, summarization and memory save. Turn/tool activation indices come from
existing guest counters; attempts remain a separate `u64`. Sequential retries
use the retry local; parallel retries read each item's saved key and attempt.
Pool-member identity and live call order do not contribute to the path.

The larger private signature requires an indirect argument record for canonical
async lowering. A small core shim owns a 40-byte record in the parent arena,
reserving 48 bytes to accommodate alignment. Its lifetime matches other pending
call input buffers. The legacy backend does not emit this shim or allocate its
records. Adapter-v2 path construction adds the base path length plus 18 bytes
per live call; its existing arena resets once all active calls return. These are
allocation shapes, **not** a replacement for measured execution or aggregate
memory results. The committed performance comparison still measures adapter v1;
repeat paired measurements for v2 and the complete backend before qualification.

Execution checks cover distinct step and parallel-item paths, stable identities
across fresh runs, checkpoint replay with zero child starts, and sequential and
parallel retries that retain one path through attempts 1/2/3. A retry fixture
supplies initial errors; the final attempt executes the actual prepared utils
Agent. Direct adapter tests preserve Unicode paths, all bits of `u64::MAX`
attempts and `u32::MAX` activation fields, repeated identities and 1 MiB raw bytes.

Verification: the full workflow suite passed **854 tests** (569 library, 231
emitted execution, 54 other integration tests); the two manual benchmarks and
existing doctest were ignored. The expanded focused set then passed all **11
checks**, including AI single-shot/tool-loop/memory/summarization composition and
protected input-variable identities. All four direct adapter ABI tests passed.
After the async argument overflow guard, the 11 focused checks passed again.
All **120 component-host tests passed**, with one manual benchmark ignored.
All 27 Agent and both shared components rebuilt successfully; integration-feature
all-target Clippy passed with `-D warnings`. AI composition checks validate the
emitted ABI and package wiring, not provider execution or all AI replay semantics.

These paths remain **relative guest metadata**. The production scope factory must
bind them to immutable tenant/root authority, validate their use and apply
persistent attempt fences. No targeted command routing or cancellation API is
being enabled by this compiler change. Full AI runtime qualification, nested
Embed extraction and final local-server testing remain required.

### Root completion waits for descendant cleanup

Inspection of the production runtime boundary exposed a gap in the earlier
supervision guarantee: delaying the returned `InvokeExit` did not delay the
root's `RuntimeHost.complete`/`fail` calls. A guest could already persist success
before an unreleased child's cleanup failed.

`execute_invoke_with_context` now stages these two callbacks in a per-run host
wrapper. All other runtime methods retain their underlying host behavior. After
the root Store and every registered descendant are reaped, the supervisor
publishes a matching terminal callback once. Identical duplicates are idempotent;
conflicting callbacks or a success payload that disagrees with the exported
result become host failures without publishing the staged terminal value. Failure
callbacks retain their original bytes, including additional error metadata.
Cleanup failure, root trap, timeout and cancellation discard staged callbacks.
The legacy entry points are unchanged.

Final publication is still supervised native IO. It uses the original execution
timeout, including time spent in cleanup, and observes root cancellation and
caller abandonment. A pending callback is dropped on those conditions; a
publication error becomes a host failure after guest execution, rather than
re-entering the completed graph. Mandatory cleanup itself is still awaited even
when the timeout expires. This cannot roll back a database request already
committed, intercept an internally composed legacy SDK runtime, or replace the
persistent generation fences required by P4. Root command acknowledgement and
the separate child runtime authority adapter remain outstanding.

Six new real-WASM fixture tests call the actual runtime imports and cover held
and failed cleanup, root trap, duplicate/conflicting callbacks, output mismatch,
raw failure payloads, late cancellation, pending-publication cancellation/timeout/
abandonment, publication errors and timeout consumption during cleanup. The full
component-host suite passed **126 tests**, with one manual benchmark ignored.
All **11 emitted Agent isolation checks** passed, as did the comparison smoke
across both backends and all 11 workload definitions. Component-host all-target
Clippy with both integration/PoC features passed with `-D warnings`. These are
host/WASM boundary tests; database fencing and the final local server gate are
not yet satisfied.

### Child runtime authority and deferred root commands

`runtara-environment::runtime_host::scoped` supplies a child runtime adapter bound
to one immutable root host, child input, logical path and the admitted task's
actual cancellation token. It implements every `RuntimeHost` method. Child
success/failure callbacks are captured locally; identical duplicates are
idempotent and conflicts remain errors. Events retain their payload/subtype and
carry the child path. Every checkpoint, retry, custom-signal and sleep address
requires an explicit host-supplied `CheckpointAuthority`; keys are preserved
without adding another namespace. There is no permissive default authority.

Sibling signal polls observe the same root command without acknowledging it.
The root owner retains exact command receipts, deduplicates observations, and
requires closure after descendant teardown before applying them through core's
atomic acknowledgement handler. A stale receipt cannot consume a replacement
command. Breakpoints are likewise deferred and coalesced; cancelling their child
discards them, and an accepted root command supersedes them. Child cancellation
does not write a root command or cancel a sibling. Closure rejects new child
runtime calls. This is a process-local admission fence, not a persistent fence
for database writes already in flight.

Verification: **31 runtime tests passed against an isolated PostgreSQL instance**,
including eight new scoped tests and the existing legacy runtime checks. The new
tests cover terminal capture, conflicting callbacks, sibling checkpoint authority,
unchanged keys/payloads, non-destructive custom signals, pause/cancel/shutdown
receipts, stale commands, targeted cancellation, breakpoint coalescing and command
precedence. One test executes a real WASM component through the prepared component
and runtime linker against PostgreSQL: child completion and pause observation
leave root status unchanged until explicit owner finalization. Environment
all-target Clippy with `db-integration-tests` and `-D warnings` passed.

The adapter is not yet connected to the production invocation-scope factory or
root runner. Its test authority uses explicit sibling prefixes; production must
validate the compiler's actual namespace contract. Root polling/finalization
integration, bounded shared polling, child outcome reconciliation, durable
attempt fencing, and the complete parking/wake protocol remain required. Linker
sleep aliases also need qualification with that production scope. These tests
do not satisfy the final local-server or all-construct compatibility gates.

### Persistence-backed invocation scope factory

`ScopedInvocationFactory` now connects the child runtime adapter to
`PreparedInvocationLauncher`. It binds host-approved input, environment, limits,
checkpoint authority, the real admitted task token and independent root
cancellation. Authorization is mandatory and runs against the parent/package
policy; there is no permissive fallback. The owner is checked again inside the
admitted task, so closing between authorization and start prevents execution.
Any fresh descendant context is transferred to the launcher's existing cleanup
supervisor. Setup failure and outcome-check failure still reap that context.

After the child Store is dropped, the launcher checks captured callbacks against
the exported outcome. Conflicting callbacks or mismatched success bytes become
host failures. Trap/cancel/timeout/suspension retain their typed outcomes and
discard the captured completion. The factory does not publish root state or
choose graph successors, retries or recovery.

The inherited absolute deadline now reaches the Store, epoch/watchdog and HTTP
deadline. Store setup cannot reset it. A new real-WASM test exposed that an
already-expired budget could complete before the first watchdog tick; invocation
now checks the active deadline before entering an initializer.

Verification: **35 database-backed runtime tests passed**, including four new
factory tests with real prepared WASM child execution, forged input identity,
callback conflicts/mismatch, closed-scope races and cancellation/deadline before
initialization. All **11 emitted Agent isolation tests** and the comparison smoke
over both backends and all 11 workloads passed. Feature-enabled all-target Clippy
passed for environment, component host and workflows. The full component-host
suite passed **127 tests**, with one manual benchmark ignored; its new launcher
test checks that setup/outcome rejection closes descendant ownership. After adding
an independent inherited-deadline check (30-second child timeout with an expired
root deadline), all **nine focused launcher tests passed**.

The tested authority policy uses explicit fixture identities. The production
compiler-backed policy, root runner/command coordination, durable attempt fences,
aggregate resource reservations and recursive child-graph extraction still need
integration and qualification. No backend default changed and the final local
server gate remains outstanding.

### Supervised root lifecycle coordination

`execute_invoke_with_coordinator` now closes the root's runtime authority after
descendant cleanup and runs root lifecycle finalization before staged terminal
publication. `ScopedRootRuntime` implements both the parent runtime interface and
that coordinator, sharing the children's receipt owner. Parent/child polls retain
the same exact command for one post-cleanup acknowledgement. Root breakpoints are
coalesced with child breakpoints. An accepted Cancel replaces completion with a
cancelled result; Pause/Shutdown suspend without discarding guest-returned wakes.
Core still owns the distinct durable Pause/Shutdown wake and reason fields.

Cleanup failure closes access but forbids lifecycle finalization and publication.
Root traps and inconsistent terminal callbacks likewise do not publish control
effects. Native cancellation/timeout during cleanup takes priority over callback
validation. Finalization IO is supervised by the original budget and root/caller
cancellation; if skipped or interrupted, pending durable commands remain for
runner/scheduler recovery. This does not fence an already committed database
request. An ordinary execution entry using the scoped root runtime cannot publish
terminal callbacks before successful coordinator finalization.

Verification: **39 database-backed runtime tests passed**, including four new
real-WASM root tests covering held cleanup, shared command observations,
Pause/Cancel/Shutdown, root completion, breakpoint coalescing, retained on-resume
wakes, cleanup failure, traps and conflicting callbacks. Shutdown's recovery wake
and reason remain distinct from Pause. The full component-host suite passed
**130 tests**, with one manual benchmark ignored; after adding native-stop
precedence coverage, all **18 focused supervision tests passed**. These include
pending finalization timeout/cancel/abandonment, close/finalization errors and
cleanup ordering. All **11 emitted Agent isolation checks** passed, as did the
comparison smoke across both backends and all 11 workloads. Feature-enabled
all-target Clippy for environment and component host passed with `-D warnings`.

This supplies the root coordinator used by the scoped execution API. The
environment runner still needs to select and construct it from the approved
artifact/authority policy. Durable root/attempt fences, aggregate reservations,
extracted child graphs and final local-server qualification remain required.
Shared polling is implemented below; the production default is unchanged.

### Compiler invocation inventory through prepared execution

Logical Agent compilation now produces an immutable inventory from the same
normalized manifest used to emit WASM, including nested graph and preloaded child
Agent references. Identities are sorted and deduplicated with their allowed
step/AI domains. Composition rejects logical lowering without its inventory and
continues requiring the exact reviewed package selection. The sidecar reports
package version 2; ordinary compilation and the v1 experimental bridge retain
their prior package formats.

Raw package v2 carries the inventory alongside digested component members. The
trusted worker preserves it in native envelope `RTRNP002`; prepared catalogs,
queued ownership and the optional prepared cache retain the same data. Raw and
native decoding reject missing or mismatched versions, invalid bindings/domains
and duplicate identities. Existing byte/count limits also bound this metadata.
Old package/native forms remain accepted without invented authority.

Verification: all **18 workflow-WIT/package tests** and **four native codec tests**
passed. The full component-host suite passed **132 tests**, with one manual
benchmark ignored. After extending the cache test to v2, it passed with cache on
and off and executed a retained actual utils child after source/cache removal.
All **11 emitted Agent isolation tests** passed with inventory checked across raw,
native and prepared representations and against actual observed call identities.
These cover chains, retries, replay, parallel items/branches, protected input
variables and legacy package selection; AI auxiliary inventory is checked at
composition. The comparison smoke also passed both backends across all 11
workloads. Provider execution qualification remains pending. No database tests
were rerun for this transport-only change; the existing scoped-runtime test
fixture only gained an absent-inventory field. Feature-enabled all-target
Clippy for all four affected crates passed with `-D warnings`.

The inventory transports static call sites; the production policy still needs
namespace/loop validation, checkpoint authority and durable attempt fencing.
No production runner or backend default has changed. Current v2 package size,
validation cost and execution timings require fresh paired measurements; the
existing v1 performance report is historical evidence for that backend only.

### Shared lifecycle polling for scoped execution

The scoped root and all children now use one lifecycle poll cache and one
in-flight read per root owner. Ordinary `check-signals`/`is-cancelled` calls share
positive, empty and error results for the root host's existing interval (one
second by default). Every sibling sees a cached pending command; a child cannot
consume the only observation. The cache retains command identity/type only.
Explicit checkpoint receipts still request a fresh read to reject superseded
commands; checkpoint IO and these validations are outside the tight-loop polling
budget. Custom-signal and heartbeat behavior is unchanged.

A checkpoint or interrupted sleep that reports a pending command invalidates the
cache immediately. Invalidation does not wait behind a slow poll, and a read
started before invalidation cannot populate a valid cache entry afterward. A
cancelled reader drops its request and releases the shared lock, allowing a
sibling to retry without spawning an unowned worker. Closure checks before and
after polling prevent a result from re-opening runtime authority after teardown.
The actual task cancellation token still wins immediately for its child.

Verification: **47 runtime tests passed** with isolated PostgreSQL, including
six deterministic poller tests and the existing real-WASM lifecycle tests. New
coverage verifies 64 concurrent waiters share one read, all callers retain a
positive observation, empty/error cache expiry, explicit receipt revalidation,
zero-interval operation, cancelled reads, and invalidation during a blocked read.
Database tests cover 48 parent/child polls, checkpoint invalidation from either
scope, replacement Cancel receipts, and Cancel/Shutdown sleep interruption in
both scopes without early acknowledgement or legacy escalation. The initial
sleep test incorrectly expected Pause to interrupt; it was corrected to retain
Core's existing cooperative Pause semantics. Feature-enabled all-target Clippy
passed with `-D warnings`. The test PostgreSQL container was stopped afterward.

This bounds ordinary lifecycle polling across the scoped execution tree. It does
not add targeted command routing, durable fencing, production runner selection,
or change Pause into a sleep interrupt. Those remain separate plan requirements.

### Enforced static invocation identity checks

The prepared launcher now checks every request for a v2 Agent package against
its verified compiler inventory before calling the scope factory. A shared
`AgentInvocationPath` decoder requires the exact canonical v2 tuple and two
fixed-width a–p counters. It decodes structured child/tool-child namespaces and
Split/While indices, rejects malformed/ambiguous encodings, and preserves authored
Unicode and delimiter characters as data. Lookup checks workflow, binding,
Agent, capability, step and domain. Attempts must be nonzero; AI auxiliary calls
use attempt 1, while only AI turn/tool domains accept activation counters.

This enforces the previously transported static inventory at the actual prepared
launch boundary. It does not authorize arbitrary well-formed namespaces: the
mandatory scope factory still needs the compiler-backed membership policy and
checkpoint grants. In particular, the flat inventory alone cannot distinguish
the same Agent/step identity in different graph scopes. The next integration must
bind those scopes to compiler ancestry before granting persistence. Durable
attempt arbitration and the production runner selector remain pending. Legacy
packages without this inventory retain their existing scope-factory contract.

Verification: **21 workflow-WIT/package tests** and **132 component-host tests**
passed, with one manual host benchmark ignored. All **11 emitted Agent isolation
checks** passed through the new launcher check. The cache-on/cache-off test now
rejects ten malformed or mismatched request cases before scope allocation,
verifies that a well-formed foreign namespace still requires the scope policy,
and executes the valid retained utils child afterward. The comparison smoke
passed both backends across all 11 workloads. No database code changed and no
database tests were rerun for this static launcher check. Feature-enabled
all-target Clippy for both affected crates passed with `-D warnings`.

The first host build hit a Serde trait mismatch in unchanged native-agent tests
while another Cargo process used the shared target directory. Dependency
inspection found one Serde version. A fresh build in the separate
`target/selective-isolation-check` directory passed the full host suite; no
shared caches were removed and no unrelated processes were interrupted.

The decoder is a host/compiler helper and adds no guest interface or DSL field.
Fresh v2 performance measurements must include its per-invocation parsing and
lookup costs; the historical v1 benchmark does not exercise this check.

### Qualified Agent definition and caller identities

A new emitted regression reproduced an address collision: a root `r0` Agent and
an `r0` Agent inside `WaitForSignal.onWait`, both calling the same capability,
produced the same v2 logical invocation path. Authored step IDs and loop ancestry
alone do not identify every nested graph definition. A shared tool Agent can
similarly be called by two different AiAgent controllers whose counters both
start at zero.

The compiler now assigns a token to each normalized Agent definition, caller and
semantic role. The emitter and package inventory use the same token assignment;
assignment includes unselected definitions so selecting an additional package
does not renumber existing tokens. AI tool calls include the actual AI caller,
while ordinary Agent, provider and memory calls use their own definition. The
inventory can represent multiple definitions with the same authored identity.

Scoped compilation now emits `logical-agent-call:3`, the private
`scoped-capabilities-v3` interface, and inner invocation inventory version 2. The
outer raw package and native transport remain version 2. The copied invocation
path changes to `runtara:v3:` and its first fixed-width suffix identifies the
call-site token; the second remains the activation counter. Retry attempt remains
a separate u64. The canonical parameter layout and durable checkpoint key bytes
are unchanged. Existing v2 paths require inventory version 1; v3 paths require
inventory version 2. Cross-version reinterpretation is rejected. Previously
compiled packages keep their existing code and interpretation.

Admission checks sorted unique tokens, valid identity and role references,
consistent definition-to-identity mapping, unique definition/caller/role tuples,
and complete role coverage. The prepared launcher's resolver applies the original
role-specific activation and attempt rules after token lookup, and checks that
the token belongs to the requested Agent identity. These static checks still do
not grant checkpoint namespace access or implement durable attempt fencing.
Compiler-backed namespace membership and production selection remain required.

Verification: **24 workflow-WIT/package tests**, **132 component-host tests**,
and **858 workflow tests** passed. The workflow suite includes 235 emitted
integration cases, all 13 Agent-isolation checks, and the paired comparison smoke
across 11 workloads. The new onWait collision regression failed before the fix
and passes afterward. A compiler test checks distinct callers for a shared AI
tool and stable tokens across package selection; this is composition evidence,
not a live isolated LLM conversation test. Native transport tests cover both
inventory versions. Bridge tests exercise Unicode, maximum counters, repeated
calls and malformed version prefixes rejected before launch. All 27 Agent and
two shared workflow components rebuilt successfully. Feature-enabled all-target
Clippy passed with `-D warnings`. Manual release benchmarks and the existing
ignored doctest were not run; no database code changed or database suite reran.

Fresh paired measurements must include the v3 bridge, larger per-call-site
inventory, and launch-time decoding. The historical v1 comparison does not
measure these costs; no new performance claim follows from this identity fix.

### Compiler-backed invocation namespace membership

The compiler now emits inner invocation inventory version 3, containing allowed
relative namespace shapes for each call-site token. Split and While bodies append
their authored loop kind and ID. An inline Embed captures the parent loop shape
in a child frame and resets the child's local loops. WaitForSignal.onWait keeps
the enclosing loop shape; its Agent definition remains distinguished by the
qualified token. Reused inline children accumulate their permitted shapes.
Definitions outside the traversed root graph closure have an empty allowed set.
This metadata contains address
membership, not runtime edges, scheduling decisions or workflow inputs.

`PreparedInvocationLauncher` checks this inventory before asking the scope
factory for runtime authority. The check requires exact frame counts, child IDs,
workflow IDs, loop kinds and loop IDs; dynamic indices retain the canonical
unsigned representation. A nested launcher may bind an immutable inherited
namespace supplied by its host parent. Its frames, including actual parent loop
indices and tool-call counters, must match exactly before the relative pattern is
checked. Request input cannot replace that inherited authority.

The raw/native envelope and v3 invocation ABI remain unchanged; inventory version
3 requires the new reader. Versions 1 and 2 retain their existing interpretation
and scope-factory requirements. Durable checkpoint key bytes and graph execution
remain unchanged. This completes the static namespace-membership check at the
prepared launch boundary. It does not establish that an iteration is currently
live, grant checkpoint IO, or wire the production runner. Per-child checkpoint
grants, durable attempt arbitration and production ownership integration remain
necessary before enabling the backend.

Verification: **26 workflow-WIT/package tests**, **132 component-host tests**,
**567 default-feature workflow unit tests**, and all **14 emitted isolation tests**
passed. The Embed test additionally executes two outer and two inner iterations
and verifies all four distinct cross-boundary addresses. Every recorded emitted
Agent request is checked with forged extra child and loop frames; rejection
occurs before the scope factory is called. Package tests cover missing/duplicate
scope declarations, old-version rejection, exact inherited tool namespaces,
foreign workflow/child IDs, and missing/extra/wrong-kind loops. Native transport
roundtrips all three inventory versions. All 27 Agent and two shared workflow
components rebuilt successfully. Feature-enabled all-target Clippy
passed. No database code or guest runtime/WIT implementation changed; database
and full legacy integration suites were not rerun for this metadata check.
Fresh size and timing measurements must include these additional inventory bytes
and per-invocation membership checks.

### Compiler checkpoint contracts

Inner invocation inventory version 4 adds one explicit checkpoint contract per
call-site token: no checkpoint IO for native capabilities, the existing child
namespace for normal workflow-agent calls, or approved AI caller/tool labels for
workflow-agent tools. The contract comes from the same normalized workflow-agent
flag that controls the guest's input envelope. Versions 1–3 keep their original
format and interpretation; the invocation ABI, raw/native envelope and durable
key bytes remain unchanged.

A shared `CheckpointNamespace` derives a non-root subtree from an authorized
invocation. Workflow-agent input must carry exactly that compiler-derived prefix;
tool calls must also match an approved label and the current activation counter.
Native input stays opaque and grants no checkpoint IO. Key checks decode the
canonical v2 tuple, compare structured namespace frames, and accept the existing
canonical retry/attempt suffixes. They do not prepend a prefix or interpret graph
operations. Descendant namespaces remain within the owned subtree.

Existing durable keys can overlap between different workflow-agent definitions,
even though their qualified invocation tokens differ. The inventory now reports
identical and ancestor/descendant grant conflicts. This is an eligibility input:
the production selector must retain the legacy backend for affected packages,
rather than silently changing checkpoint keys or permitting overlapping isolated
grants. That automatic fallback is still pending.

Verification: **30 shared contract/package tests** and **132 component-host tests**
passed. The **16 emitted-isolation checks** include agreement with the existing
stdlib input-scoping helpers and a compiler regression for matching Agent IDs in
root/onWait graphs: invocation tokens remain distinct while the conflicting
checkpoint grants are detected. Renaming the nested step removes the conflict.
Native transport tests roundtrip all four inventory versions. All 27 Agent and
two shared workflow components rebuilt successfully. Fresh size and latency
measurements must include the additional contract metadata and grant validation.

### Compiler-backed persistence authority

`CompilerInvocationAuthority` now implements the environment's mandatory scope
policy using the exact prepared catalog and an immutable host-supplied inherited
namespace. It requires inventory version 4, rejects overlapping checkpoint grants
before creating children, revalidates each invocation, and derives checkpoint
permissions from its compiler contract. An input envelope can confirm the
expected workflow-agent namespace but cannot choose a different grant. Native
capabilities receive no checkpoint permission.

The resulting authority plugs directly into `ScopedInvocationFactory`. Every
checkpoint read/write, custom-signal poll, durable sleep and retry report passes
through the structured grant on `ScopedRuntimeHost`. Parent/foreign namespace
operations fail before persistence access. Child input, tenant/instance ownership,
root deadlines, cancellation tokens and terminal-callback validation retain the
existing scoped runtime behavior. The factory creates no descendant execution
context for these Agent calls; recursive extracted child graphs remain P3 work.

Verification: all **48 runtime-host tests** passed against isolated PostgreSQL.
The new integration test exercises native, normal workflow-agent and AI-tool
contracts through the actual scope factory, checks allowed checkpoint/signal/
sleep/retry operations and denied foreign operations, verifies denied writes do
not appear in persistence, and executes a real WASM child through the prepared
launcher. Its completion leaves the root running. Old-inventory and overlapping-
grant catalogs are rejected by this new authority; existing explicit legacy
scope policies retain their original path. Feature-enabled all-target Clippy
passed with warnings denied. The test PostgreSQL container was stopped afterward.

This supplies a concrete persistence policy; it does not enable a production
backend. The selector must still certify component reset behavior and structured-
key support, retain legacy execution for unsupported or conflicting packages,
and bind the factory/root coordinator to the production runner. Durable attempt
fencing is still required for database requests already in flight at cancellation.
Local-server qualification and fresh performance comparisons remain pending.

## Remaining required work

- P0: extend explicit differential selection and invocation-count evidence to all
  required constructs and production artifact inspection.
- P1: conflict-aware package eligibility and production scope-factory wiring, aggregate
  input/transport/guest resource reservations and root fencing on cleanup failure.
- P2: qualify logical scopes across every AI auxiliary invocation and nested
  construct, integrate the production scope factory and certify package reset/state
  eligibility. Basic emitted Agent paths and attempts are wired above.
- P3: recursive Embed extraction and production scoped-runtime integration, suspension/wake sets,
  scopes, deadlines, checkpoint keys and existing reference ABI modes.
- P4: durable attempt transitions, production root-coordinator and targeted command routing, crash/lease
  fencing, resource/tenant ownership and parked invocation handling.
- P5: all compatibility gates, extend the paired Agent measurements to direct
  step spans, aggregate resources and production qualification; full unit and
  integration suites, local server plus isolated persistence E2E.
- P6: controlled opt-in and artifact-compatible rollback; no default enablement
  before all gates above pass.

No local server has been launched yet. The experimental emitted Agent path has
correctness and paired local performance evidence. It does not yet establish
durable targeted cancellation or completion of the full plan.
