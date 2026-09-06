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
grants. The review-driven compiler selector described below now applies this
fallback; production selection remains pending.

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
backend. The compiler selector below now requires explicit reviews of reset
behavior and checkpoint IO, and retains legacy execution for unsupported or
conflicting packages. Production still needs package reviews and must bind the
factory/root coordinator to the runner. Durable attempt
fencing is still required for database requests already in flight at cancellation.
Local-server qualification and fresh performance comparisons remain pending.

### Review-driven compiler selection and legacy fallback

`compile_direct_workflow_composed_with_isolation_policy` resolves actual graph
dependencies through the existing component and workflow-agent safety gates,
selects packages before WASM emission, and composes that exact selection. It
emits the workflow once. Existing compilation APIs and exact-selection tests
retain their previous semantics; the production default is unchanged.

The embedding supplies an explicit policy and asserts that its chosen runner
supports inventory v4 and compiler checkpoint authority. Each dependency needs
a review of its exact SHA-256 bytes, approval of fresh per-call stores, and
approval of the compiler's checkpoint contract. The latter means no checkpoint
IO for native capabilities, or structured v2 keys beneath the supplied namespace
for workflow-agents. These are trusted operator reviews, not workflow-controlled
flags or automatic claims about an agent's behavior. Unknown and stateful
packages without that approval keep their original component lifetime.

The sidecar's optional `isolationSelection` report lists actual dependencies in
canonical order, resolved and reviewed digests, and the first failed gate:
disabled policy, unavailable runtime, missing review, unapproved reset behavior,
unapproved checkpoint contract, changed digest, unsupported invocation contract,
or checkpoint overlap. Successful entries are marked `isolated`. The report
exists even when every package falls back. Old APIs omit it; fallback does not
add custom sections or change emitted legacy WASM bytes. Unrelated review IDs
are ignored and never become filesystem paths.

Conflict checking includes all workflow-agent definitions, including dependencies
left on the legacy path. It also includes inline Embed namespaces whose child
graphs contain no Agent calls. A grant that could contain an inline child's
checkpoints forces its whole Agent package onto legacy execution. Unrelated
eligible packages can remain isolated. Existing checkpoint keys are preserved.
Missing or inconsistent component artifacts remain errors. Composition rechecks
the selected dependency identities; changed bytes fail before replacing the
previously composed artifact.

Verification: **567 compiler unit tests** and **21 emitted-isolation tests**
passed. Five new integration checks cover the basic fallback gates with exact
legacy logic/component byte comparisons; real mixed execution and child counts;
same-package and cross-package grant aliases; inline Embed aliases and a renamed
non-conflicting control; sidecar roundtrips; changed bytes after selection; and
invalid artifact rejection even with the policy disabled. Feature-enabled
all-target Clippy passed with warnings denied. Component fixtures use existing
staged components; no guest runtime or Agent component implementation changed
in this increment.

This API is not yet called by the production compilation service. Production
runtime capability negotiation, policy-aware cache keys, package review storage,
and runner construction remain required before enabling it. Fresh benchmarks
must use the complete scoped runtime path; historical adapter-v1 measurements
do not measure this selector or compiler checkpoint authority.

### Opt-in EmbeddedWasmRunner execution

`EmbeddedWasmRunner::with_scoped_agents` now admits and executes reviewed Agent
packages through the real runner. Its explicit configuration pins approved
Agent IDs and component digests and bounds child task slots, retained result
bytes, and canonical handles per root. Approval covers fresh-store behavior and
the compiler checkpoint contract. The constructor and server startup still
leave this path disabled; ordinary artifacts keep their existing execution.

Preparation verifies the package before reading persisted input or taking an
active run permit. It requires inventory v4 without checkpoint grant conflicts,
matching runtime reviews, a root from the runner's engine, a lifecycle export,
and native runtime imports. Roots exposing raw WASI HTTP imports are
conservatively excluded because their persistence cannot be assumed to pass
through the supervised native runtime. Preparation is not a recompilation point:
incompatible packaged artifacts fail admission. Compiler-side fallback remains
responsible for producing legacy artifacts where isolation is unsuitable.
Admission is checked again when consuming a prepared token, covering a changed
runner policy and a token passed to another engine.

After the start gate opens, the runner constructs `ScopedRuntimeOwner`, the
compiler authority, `ScopedInvocationFactory`, prepared launcher, task registry,
and a fresh execution context. All children inherit the root's approved
environment, cancellation signal, memory/table limits and one absolute active
deadline. Queue/gate waiting does not consume that active budget. The root uses
`ScopedRootRuntime` as both its runtime and coordinator; the existing supervised
execution API closes children and finalizes root control before publishing a
staged terminal callback. The detached runner retains its run slot through that
await and then applies its existing cancellation/suspension handling.

The new `scoped_runner_test` suite compiles real DSL through the review-driven
compiler API, prepares the actual package through the precompile protocol, and
runs it against PostgreSQL using `EmbeddedWasmRunner`. Six tests cover approved
and legacy execution, disabled/unreviewed/changed/old/runtime-less admission,
prepared-policy withdrawal and foreign engines, start-gate timing, durable
suspension and exact checkpoint replay, and a hung HTTP child stopped by either
`Runner::stop` or the root deadline. The HTTP fixture remains blocked until the
workflow has exited; releasing it afterward does not publish root success.
This confirms whole-workflow interruption through the scoped runner, not durable
cancellation of one step while the rest continues.

CI now runs this suite in `components-build`, with staged components and an
isolated PostgreSQL service. Its feature is also included in the authoritative
lint/build matrix. Local verification passed the six new integration tests,
five existing embedded-runner integration tests, 23 embedded-runner unit tests,
and 48 runtime-host tests against isolated PostgreSQL, plus 103 component-host
unit tests. Feature-enabled all-target Clippy passed with warnings denied. The CI YAML and feature wiring were parsed
and checked locally; the remote CI job has not been run for this commit.

The following section covers server startup policy, shared compiler/runner review
configuration, compatibility fallback and compilation cache identity. Per-root
task/result/handle bounds do not
establish aggregate input, guest memory, transport or descendant quotas; reported
runner memory remains the root Store's metric. Durable attempt fences, targeted
commands, recursive extracted children, fresh benchmarks, Linux/capacity gates,
and final local-server testing remain required before rollout.

## Shared server policy and artifact-compatible rollback

The server can now opt in through `RUNTARA_EXPERIMENTAL_ISOLATION_POLICY`, a
path to an operator-owned JSON file. It loads and validates the file once during
configuration initialization and shares that snapshot between normal workflow
compilation and the embedded runner. An absent policy preserves legacy behavior.
This remains an experiment, not default enablement or approval of any package.

Example schema (replace the placeholder with the SHA-256 of reviewed component
bytes; do not approve a package merely because its digest matches):

```json
{
  "version": 1,
  "compileEnabled": true,
  "reviews": {
    "utils": {
      "sha256": "<64 lowercase hexadecimal characters>",
      "resetSafe": true,
      "compilerCheckpointContract": true
    }
  },
  "retainedReviews": {},
  "maxChildTasks": 8,
  "maxResultBytes": 8388608,
  "maxHandles": 32
}
```

Both approval flags require review, including for checkpoint-free native agents.
Unknown fields, unsupported versions, invalid digests and invalid resource bounds
fail startup. Quotas are per root and do not establish aggregate descendant,
transport or guest-memory limits. The compiler's package byte limit matches the
native precompiler admission limit; it is separate from retained result bytes.

The normal `compile_workflow_direct` API accepts an optional policy and package
limits. The server supplies them from the snapshot. Eligibility is resolved before
emission; legacy remains the fallback for unreviewed or incompatible packages.
For policy-driven compilation, a root without the lifecycle/native runtime shape
(including composed runtime binding or omitted runtime), or a remaining shared or
legacy dependency importing raw `wasi:http/`, prevents isolated selection. The
report records `unsupported-root-runtime`. HTTP imports in a selected child alone
do not exclude the root. Exact experimental selection APIs retain their existing
behavior; the runner still validates the resulting artifact before admission.
Shared component digests inspected during selection, as well as all Agent digests,
are rechecked before composition replaces an existing output.

Enabled compilation adds a deterministic fingerprint to the existing lowering
mode provenance. It covers current package reviews, their approval flags, root
runtime binding, invocation ABI and inventory version. Image reuse, immutable
image names, successful artifact freshness, queue claims and recorded-failure
freshness use that same tag. Runtime-only quotas and retained reviews do not
change emitted bytes and do not invalidate compilation caches. No SQL migration
is required. Policy changes take effect after server restart; the file is not
reread during requests.

To stop producing new isolated artifacts, set `compileEnabled` to `false` and
restart. This restores the legacy compile path and cache tag while keeping exact
runtime approvals active. When replacing a current review, put its previous full
review object into `retainedReviews[agentId]`, an array, for as long as artifacts
pinned to those bytes must run or resume. Retention approves only that Agent ID
and digest. Removing the policy or withdrawing both current and retained approval
makes such artifacts fail admission; rollback must retain their reviews. It does
not rewrite a pinned package or turn it into a legacy artifact.

The accompanying tests cover shared-policy validation/fingerprints, historical
runner approvals with a real emitted workflow and PostgreSQL, root import
classification, unsupported ABI byte parity, shared-component mutation rejection,
and the public compile wrapper's selected and legacy output. Local verification
passed 569 workflow unit tests, 24 emitted isolation tests, seven scoped-runner
integration tests, four policy unit tests, 22 server compilation-service unit tests
and 18 database-backed compilation provenance tests (644 total). Feature-enabled
all-target Clippy for the server, workflows and environment passed with warnings
denied. The server database suite initially could not run migrations on the
runner's plain PostgreSQL image because `vector` was missing; the rerun passed
against a separate `pgvector/pgvector:pg16` database matching server CI. No guest
component code changed; the emitted tests used the previously built components.
Targeted interruption, recursive children, aggregate quotas and final local-server
qualification remain outstanding.

## Transactional invocation fencing contract

Core now exposes an optional `Persistence::invocation_fences` capability, with
independent in-memory and PostgreSQL implementations. PostgreSQL migration 025
adds root leases and invocation control records. These records contain no result
payloads and do not alter existing checkpoint keys. Existing persistence methods
and runtime paths continue to behave as before; the scoped runtime has **not yet
been switched to these methods**.

A root lease contains trusted tenant/root identity, a host launch ID and a
store-allocated epoch. Recovery can inspect the tenant-owned lease record and then revoke/claim using
its exact epoch; a read snapshot does not bypass the transition checks. Claims
require a running root. Replacing an owner requires
exactly its revoked epoch; an active owner cannot be silently displaced. Repeated
claim requests return the same lease. Revocation fences future writes but does
not stop a native task or release its memory/capacity: runner teardown and join
remain required before resources are reclaimed.

The store allocates invocation generations independently of guest retry counters.
Admission retains a host start ID so retries of the same request return the same
attempt. Another active start at the same logical path is rejected. A settled
invocation can run again with a fresh start/generation; this is control state,
not success memoization. After lease revocation, a new owner can reclaim an
abandoned active path. A cancellation winner instead returns its retained
logical-path tombstone on replay, before a Store or external IO should start.
An old generation cannot cancel a later generation, and exact cancellation does
not imply prefix/group cancellation of descendants.

Cancellation and settlement serialize with fenced checkpoint writes. Settlement
may atomically commit an existing logical checkpoint and the attempt's terminal
control state. Cancellation wins prevent those bytes from being written;
settlement wins make late cancellation a no-op. Read-or-insert preserves the
first committed checkpoint bytes, returns them on a hit, and treats an empty
payload as a probe. A settlement response carries any existing checkpoint value
so callers can honor replay semantics. Root checkpoint pointers update in the
same transaction as insertion. Retention deletes leases and tombstones with the
root instance.

Every PostgreSQL operation first locks the tenant-owned root instance row. This
serializes it with root terminal transitions as well as other fenced mutations.
Opaque path equality is checked in full; hash indexes avoid btree entry-size
limits for long paths without treating hashes as identities. Operations under
this contract serialize per root and retain attempt history. Their latency,
contention and retention cost must be measured and bounded before rollout.
Ordinary non-durable invocations must continue using live in-memory arbitration;
adding this optional capability introduces no automatic per-step database IO.

Eight shared contract cases cover ownership, request idempotency, concurrent
admission, cancellation/replay, settlement/checkpoint semantics, lease takeover,
cancellation races and retention against both backends. PostgreSQL adds a forced
settlement failure after checkpoint insertion to prove rollback, and an observed
blocked-writer test: the test checks that the writer's exact backend PID is
waiting on the controlled root lock, commits cancellation or revocation, and
verifies that no late checkpoint or pointer is published. Tests use an isolated
local database. Fresh parallel setup exposed a `CREATE EXTENSION IF NOT EXISTS`
race; shared test initialization now serializes extension creation.

Verification passed 76 core unit tests and the full PostgreSQL suite (72 backend
unit tests plus 18 conformance/integration tests), 166 tests total. The 18-test
suite also passed with parallel execution against a newly created database.
Feature-enabled all-target Clippy passed with warnings denied. The existing
migration documentation example remains explicitly ignored by Rustdoc; no new
check was skipped. These are library/persistence tests, not local-server E2E.

Required integration remains: bind leases to runner launch/recovery ownership;
admit/settle attempts asynchronously around real child execution; fence all
child write families (including retry records, sleep/wakes and events); arbitrate
parent checkpoint/result publication; route deduplicated tenant-scoped targeted
commands; expose runtime invocation addresses; and preserve non-durable behavior.
These primitives alone do not enable user cancellation of one step, remove E128,
or establish crash-safe production execution. Local-server E2E and fresh
performance comparisons remain required.

## Supervisor-owned asynchronous admission and settlement

The native task registry now accepts an optional `TaskLifecycle`. Admission runs
before the execution factory, Store instantiation or guest initializer. It can
return a retained control outcome without constructing a Store. Settlement runs
under the supervisor after the execution future is destroyed and descendant
cleanup finishes. `PreparedInvocation` and `ChildInvocationScope` carry this
ownership through the normal execution imports and prepared launcher.

Settlement is retained even when cancellation drops admission after a database
commit but before the reply is recorded. It also runs for pre-start cancellation,
admission errors, execution panics and failed descendant cleanup. The embedding
must retain an idempotent admission identity outside the cancellable future and
bound its persistence/cleanup work. Task result publication, join/release and
capacity release wait for settlement. A settlement failure or panic closes the
result channel and makes shutdown report a persistent host failure; a hook cannot
turn failed admission/cleanup into successful execution. An admission panic is a
host failure because its external effects may be uncertain.

For tasks with a lifecycle hook, its arbitration result is authoritative. A local
cancellation arriving during settlement remains a request; it cannot overwrite a
durable completion winner afterward. The hook must resolve that race through its
persistence/control authority. Tasks without a hook retain the existing local
cancellation-wins-before-publication semantics. A managed result exceeding its
retention budget reports host failure, rather than replacing a potentially
committed outcome with a recoverable guest trap. The caller must still fence the
root when settlement or resource cleanup cannot be confirmed.

Six native tests cover admission bypass, cancelled admission, cleanup/settlement
ordering, retained capacity, late cancellation, errors/panics and result-budget
failure. Three PostgreSQL-backed tests execute a real prepared WASM child through
the new bridge. They prove that normal execution is bracketed by durable
admission/settlement, cancellation can recover an uncertain admission reply
without starting the child, and a persisted cancellation can beat a computed
result and prevent the child's initializer on replay. Generic native cancellation
settles the attempt without creating a permanent user-cancel tombstone, preserving
the distinction needed for pause/replay. Child completion never terminalizes the
root in these tests.

Verification passed 109 component-host unit tests, 28 scoped-runtime tests against
PostgreSQL, 24 emitted-workflow isolation tests and seven scoped-runner integration
tests (168 total). Feature-enabled Clippy and the workspace commit-hook lint
passed. The existing guest components were reused because no guest/WIT
implementation changed. The production scope factory installs no lifecycle hook
yet: the database adapter in these tests validates the bridge, not full production
fencing. Required work remains to guard every child write family, bind/revoke root
leases in the runner, preserve non-durable call behavior, and route targeted
commands. This change does not enable Agent/Embed timeouts or a Cancel-step API.

## Fenced child checkpoint, retry and event writes

The optional persistence contract now covers the remaining in-process child write
families. Memory uses the same store lock for validation and mutation; PostgreSQL
holds the tenant-owned root row lock through the write and commit. All four write
families reject cancelled/settled attempts, revoked or superseded leases, stale
attempt generations, inactive roots and forged tenant/path/start identities.

`invocation_sleep_checkpoint` preserves the existing sleep upsert, including literal
empty state and the PostgreSQL timestamp refresh, and updates the root checkpoint
pointer in the same transaction. It does not park the root, wait, or schedule a
wake. `invocation_retry` preserves the synthetic retry key and PostgreSQL retry
metadata, without moving the root pointer. It rejects zero/overflowing retry
counters and oversized derived keys before writing. `invocation_event` appends
telemetry at the trusted attempt's root/path; its type can only represent Custom
or Heartbeat events. It cannot terminalize or suspend a root. Observer notification
belongs after successful persistence in the future runtime integration.

Three shared conformance cases exercise successful semantics, ten categories of
lost/forged authority, and input boundaries against both backends. PostgreSQL tests
also verify retry metadata and rejected late overwrites, rollback of a sleep
checkpoint when its pointer update fails, and the successful sleep upsert's empty
state/timestamp behavior. The observed database-lock test now covers checkpoint,
sleep, retry and event writers against both cancellation and lease revocation.
Each writer is confirmed blocked at the database before the fence commits; no
late checkpoint, retry record, event or pointer may appear.

Verification passed 79 core unit tests and the full PostgreSQL suite (72 backend
unit tests and 23 conformance/integration tests), 174 total, plus feature-enabled
Clippy. An initial parallel full-suite run exposed interference in an existing
global active-instance-count assertion. The rerun used CI's `--test-threads=1`
setting; concurrency inside the race tests remains enabled. The existing migration
Rustdoc example is still ignored. No local-server E2E has run for this change.

These are additional persistence primitives, not production activation. Scoped
runtime calls and in-process sleep heartbeats still need to use these methods;
root/parent writes and launch/recovery ownership still require lease integration.
No new migration, guest component or default-path database call is introduced.
The performance plan now explicitly requires transaction counts/timing, root-lock
contention, ledger growth/retention, and a separate non-durable zero-ledger-IO check.

## Runtime IO bound to supervised durable attempts

`InvocationIo` now carries an admitted attempt's fence through scoped checkpoint
reads/writes, retry records, custom events and heartbeats. The explicit
`ScopedInvocationFactory::prepare_fenced_child` entry point applies the existing
binding/checkpoint authority and attaches the same object to the child runtime
and its `TaskLifecycle`. It rejects an IO authority from another persistence
handle, root or logical path. The ordinary factory entry point still uses live
scopes and creates no invocation lease.

The lifecycle revalidates the already-admitted start identity before constructing
a Store, then settles after execution and descendant cleanup. Persisted
cancellation suppresses a child's attempted success without becoming a storage
failure. Other persistence failures are sticky: a guest cannot catch an error and
publish success. Failed admission, IO or cleanup causes settlement to attempt
root-lease revocation and return host failure. Each control transaction has an
explicit timeout. If revocation also times out/fails, shutdown remains failed;
the runner must retain ownership/capacity and retry fencing before release or
relaunch. Initial admission, uncertain admission replies, root ownership and
recovery remain the embedding's responsibility.

Fenced sleep uses the shared core polling interval and preserves in-process sleep,
literal empty checkpoint state, zero-duration behavior and non-consuming command
polls. Each heartbeat is fenced; cancellation or revocation therefore prevents a
late heartbeat instead of letting it keep an obsolete attempt alive. A fenced
heartbeat storage failure stops the attempt and sets its failure latch. No root
wake or terminal transition is created. Checkpoint responses preserve first-write
replay and the missing-probe/custom-signal distinction. Event observers run only
after successful persistence.

Nine new tests cover those runtime semantics, denial of late writes, failure
latching even when execution returns success, cancellation overriding a caught
error, bounded control IO under a held database lock, default-path lease absence,
and real prepared WASM execution through the explicit factory. The WASM test
also verifies initializer suppression for cancellation/revocation and preservation
of the factory's authority checks. Verification passed 60 runtime-host tests
against PostgreSQL, seven scoped-runner tests and 24 emitted isolation tests (91
total), plus feature-enabled Clippy. Guest components were reused; no guest or WIT
implementation changed.

Production selection is still pending: trusted compiler durability metadata must
select the explicit path, the runner must own/revoke root leases, and parent/root
writes need fencing. These APIs do not yet expose targeted commands, remove E128,
or complete local-server and performance qualification.

## Compiler-owned invocation durability

Inventory v5 records the normalized effective durability of each emitted caller.
This preserves graph/step overrides and distinguishes same-named definitions in
nested graphs. AI tool dispatch uses the AI caller's setting because it bypasses
the target Agent step's execution plan; memory, summarization and MCP auxiliary
definitions retain their owning AI step's setting. The metadata authorizes attempt
fencing, not result memoization or additional checkpoint IO.

Raw and native package validation require exactly one Boolean flag per call token
and reject missing, extra, substituted or duplicate entries. Inventory v4 remains
readable and executable with unknown durability. The explicit fenced scope factory
accepts only compiler-authorized `Some(true)`; neither unknown durability nor
`durable: true` fields in runtime input can enable it. Ordinary preparation retains
its unfenced path for every setting. The server's compilation provenance includes
the new inventory version so fresh output cannot reuse a v4 compilation cache
entry. The guest ABI, adapter v3, raw package v2 and native `RTRNP002` stay unchanged.

Four new compiler tests cover graph/step inheritance, duplicate nested identities,
all six AI call domains, static Embed children and WaitForSignal callbacks. Package
tests cover malformed flags and old-format compatibility; native package transport
round-trips v1 through v5. A PostgreSQL-backed authority test checks all three
durability states, and the scoped runner executes legacy, current and historical
v4 packages. These tests do not activate production initial admission or root
leases. Non-durable zero-ledger-IO and current-format performance measurements
remain qualification requirements when that integration is added.

## Runner completion and capacity ordering

The scoped-runner suite exposed a race where exit was observable before the run
permit returned. The completion guard now owns the permit and releases it before
publishing completion or notifying waiters, including panic and pre-start task
destruction. A regression test blocks registry cleanup at the publication boundary
and checks available capacity; temporarily restoring the old ordering made this
test fail. The corrected ordering passes all 13 embedded-runner unit tests and
seven scoped-runner integration tests.

Verification for the durability metadata and runner fix passed 31 package/WIT unit
tests, 573 workflow compiler unit tests, 109 component-host unit tests, 38 scoped
runtime tests against PostgreSQL, 24 emitted isolation tests, four server policy
tests, 13 embedded-runner unit tests and seven scoped-runner integration tests
(799 distinct tests). Feature-enabled all-target Clippy passed. Existing guest
components were reused because no guest implementation or WIT interface changed.
No local-server E2E or new performance measurements ran for these changes.

## Remaining required work

- P0: extend explicit differential selection and invocation-count evidence to all
  required constructs and production artifact inspection.
- P1: qualify the shared server policy through local-server E2E; implement aggregate
  input/transport/guest resource reservations and root fencing on cleanup failure.
- P2: qualify logical scopes across every AI auxiliary invocation and nested
  construct, qualify the runner scope factory for workflow-agents and certify package reset/state
  eligibility. Basic emitted Agent paths and attempts are wired above.
- P3: recursive Embed extraction and production scoped-runtime integration, suspension/wake sets,
  scopes, deadlines, checkpoint keys and existing reference ABI modes.
- P4: integrate the transactional lease/attempt contract with production child IO,
  root coordination and targeted commands; qualify crash/recovery, resource
  ownership and parked invocation handling.
- P5: all compatibility gates, extend the paired Agent measurements to direct
  step spans, aggregate resources and production qualification; full unit and
  integration suites, local server plus isolated persistence E2E.
- P6: controlled opt-in and artifact-compatible rollback; no default enablement
  before all gates above pass.

No local server has been launched yet. The experimental emitted Agent path has
correctness and paired local performance evidence. It does not yet establish
durable targeted cancellation or completion of the full plan.
