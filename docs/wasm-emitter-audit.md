# WASM emitter audit

Audited 2026-09-05 against `cdcf9ee4ee0e5f28c0600b524c4984f89cbfe700`.
Scope: DSL validation, direct-WASM manifest/planning/lowering, JSON stdlib,
and durable suspend/resume through the production invoke ABI.

## Current progress · 2026-09-08

Progress checkpoint against implementation commit `62e051ed5001c28936381b4a544665be62f32e1e`
on the separate branch `feat/cooperative-cancellation-progress` (remote:
`origin/feat/cooperative-cancellation-progress`). Implementation through this
commit is committed and pushed. This documentation checkpoint adds no runtime
changes; the AI memory/MCP follow-up described below has not been implemented.

**Implementation is in progress; this snapshot is not release qualification.**
The current approach is cooperative cancellation in one normally composed
workflow `.wasm`. Workflow control and scope/deadline decisions remain in the
guest, using standard Component Model asynchronous calls and cancellation.
The platform delivers lifecycle signals, provides I/O and timers, and enforces
emergency whole-execution abort. The earlier per-step Store/task experiment is
superseded as the cancellation design; its historical evidence remains below.

Implemented and covered by focused tests:

- All 27 built-in Agents use the shared `agent_component!` export/dispatch
  machinery. The 18 I/O-capable Agents await cancellable operations.
- Root cancellation and parent propagation clean up sequential/parallel calls;
  emitted loops and retry waits have cooperation points. Normally composed
  workflow-agents, inline Embed and AI provider/tool paths have execution coverage.
- Guest deadline ownership covers Agent, Embed and enclosing loop scopes,
  including parallel calls and connection preparation. Emergency alarms bound
  uncooperative entry/cleanup. AUDIT-20 adds continuous enclosing-scope coverage
  through assembly and checkpoint work, original-clock grace restoration, and
  disposal before untimed continuation.
- Stop/cancel integrates with the environment lifecycle and has runner plus
  PostgreSQL coverage. Accepted terminal outcomes are preserved; emergency abort
  does not fabricate a guest cleanup acknowledgement.
- Authenticated Stop closes HTTP while waiting for headers or a response body,
  both on the owning server and through a peer. The original peer-startup bug
  and subsequent HTTP 500 regression now pass the four-case server E2E
  (AUDIT-23/26/27). Ownership uses retained launch leases; remote emergency grace
  is delivered through the existing physical registry and dispatcher pass.
  Success requires an armed timer or an accepted terminal outcome.
- Runner control now uses a unique physical handle for each accepted handoff,
  even when pre-start recovery reuses a durable launch ID. Stale Stop/grace/wait
  calls and old occupancy cleanup cannot target the replacement (AUDIT-25).
- The unreachable concurrent Split retry emitter is retired. Existing sequential
  retry/replay behavior remains covered, with 32 byte-identical before/after
  compiler artifacts and passing parallel/retry execution suites (AUDIT-22).

**Public Agent/Embed timeout syntax still returns E128.** Internal timeout
fixtures deliberately bypass that validation rejection to exercise the emitter;
their success does not mean users can already author those timeouts. AUDIT-28
fixes the Agent-as-AI-tool path, which previously only injected a capability
argument. AI memory and synthetic MCP provider metadata still drops the
referenced Agent budget, so blanket public acceptance would remain incorrect. No product
feature flag or optional cancellation backend has been added.

### Remaining work to reach the goal

| Work | Completion criterion |
| --- | --- |
| Behavior qualification and fixes | Complete real composed-execution race coverage, timeout/cancellation across existing retry fallbacks and deeper mixed recovery cases; qualify CPU-agent cooperation and durable nested suspension/replay against the supported construct matrix. AUDIT-21 covers deterministic parallel deadline selection; existing retrying Split/branch graphs retain sequential fallback. |
| Public timeout support | Agent-as-AI-tool budget enforcement is implemented (AUDIT-28). Carry and enforce the referenced Agent timeout for AI memory and synthetic MCP provider calls, then remove E128 deliberately and move deadline coverage through public validation/compilation/composition, including zero/overflow/inherited budgets, replay and cleanup escalation. |
| Server and persistence E2E | Authenticated owner/peer header/body cancellation passes with ownership and remote grace delivery (AUDIT-27). Complete owner disappearance/recovery, partition/clock behavior, concurrent transition and wider load qualification; preserve signal acknowledgement versus emergency-abort semantics. |
| Final measurements and capacity | Run controlled paired baseline/candidate measurements for raw/compressed `.wasm` and native artifact size, random-double single-step and full-workflow execution, cold/warm startup, cancellation/abort latency, signal/DB cost and memory. Complete Linux latency/throughput and repeated-cancellation resource soak. Earlier reports predate the latest implementation. |
| Compatibility and obsolete-path cleanup | Inventory registered/parked artifacts; retire superseded isolation/task machinery while preserving required old-artifact execution and replay contracts, runtime capability checks and export/no-host modes. |
| Upstream integration and PR | Integrate recent upstream `main`, resolve the conflicting committed migration numbers 025/026 without silently rewriting migration history, rerun affected checks and create the PR. This snapshot branch does not claim that integration is complete. |

The governing acceptance criteria remain G1–G10 in the
[updated plan](selective-isolation-plan.md). The
[implementation record](cooperative-cancellation-implementation.md) contains
per-stage tests and historical measurements. Later entries supersede earlier
"remaining" statements when they record the corresponding implementation and
evidence; the table above is the consolidated current work list.

### Latest verified implementation checkpoint (`62e051ed`)

| Stage | Recorded verification | Scope and limits |
| --- | --- | --- |
| Agent-as-AI-tool deadlines (`62e051ed`, AUDIT-28) | 682 feature-gated compiler library tests passed in 412.38s; 19 public AI execution tests passed in 14.00s; feature-gated all-target Clippy passed | The seven new Agent-tool regressions are included in the 682, not additional tests. Timeout regressions use private emission while E128 remains. Public AI tests cover currently accepted workflows. |
| Physical ownership and remote emergency grace (`b932f1c6` / `d3fc2a6a`, AUDIT-26/27) | 354 distinct selected environment tests passed across the recorded runs; authenticated owner/peer HTTP header/body E2E passed | Extended E2E also observed lease renewal during a 32-second pending request and shutdown of an unrelated peer. This is lifecycle evidence from the earlier native stage, not a fresh run against `62e051ed` or a performance benchmark. |
| Documentation checkpoint | Source and recorded results reviewed; diff whitespace checked | Rust tests, component builds, server E2E and benchmarks are not rerun for this documentation-only update. The normal pre-commit hook remains enabled; it skips Rust checks when no Rust files are staged. |

The immediate implementation work is to preserve the referenced Agent's timeout
and definition identity when constructing `memory.load`, `memory.save` and
synthetic MCP search/invoke metadata. These entries currently set `timeout: None`.
Memory calls also need shared guest budget/checkpoint handling; synthetic MCP
calls should reuse the Agent-tool deadline path. Durable call identities must
keep memory operations distinct from user-defined tools and preserve pending
budgets and completed results across replay. This remains design work, not a
completed fix or a new host task-management API.

Required regressions for those paths include zero/maximum budgets, cancellation
while I/O is pending, root/enclosing deadline propagation, independent subsequent
calls, completed and pending replay, malformed persisted budgets and unchanged
provider errors/capability arguments. After those pass, public enablement must
exercise validation, import inference, compilation, composition and supported
export modes together. E128 must not be removed solely on private-emitter results.

The full G1–G10 matrix, final paired size/performance report and Linux soak remain
unfinished. Recent upstream `main` has not been integrated into this branch;
the committed migration-number conflicts and a new PR remain outstanding. The
previous audit PR #227 was merged and is not a PR for this implementation branch.

### Verification for the scope-alarm snapshot (`f8d2d5b5`)

The current scope-alarm change has passed 598 default-feature compiler library
tests and a 670-test feature-gated library run. The expanded six-case untimed
continuation test and the subsequently added disabled-loop-import test also
passed separately; the feature-gated library now contains 671 tests. The earlier
full run therefore must not be described as one final 671-test run.

Feature-gated all-target Clippy, formatting and diff whitespace checks passed.
The final workflow execution suite passed **395 tests** in 903.51s, with three
manual benchmarks ignored and no failures. The commit uses the repository's
normal pre-commit hook, which requires formatting and workspace all-target Clippy.
The previous committed stage passed 395 workflow execution tests and 92 component
cancellation tests, and built all 27 Agents plus both shared components. Those
previous results are historical evidence, not a rerun of this snapshot. No new
Agent/WIT/stdlib source changed in the scope-alarm stage. Final paired benchmarks,
Linux soak and authenticated multi-owner server E2E have not been run for this
snapshot; database lifecycle tests were not rerun for this compiler-only change.

Commands use the pinned toolchain, `RUSTC_WRAPPER=`, `SQLX_OFFLINE=true`,
`CARGO_BUILD_JOBS=4`, an isolated native target and the existing separately built
component directory:

```sh
cargo test -p runtara-workflows --lib
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib scope_alarms_dispose_before_untimed_success_and_error_continuations -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --lib disabled_loop_budgets_do_not_add_alarm_imports -- --test-threads=1
cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute -- --test-threads=1
cargo clippy -p runtara-workflows --features direct-wasm-integration-tests --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

## Update history

**Update 2026-09-06:** AUDIT-01 is committed as `2b6bf542` and AUDIT-02 as
`d787556e`, AUDIT-03 as `a0629d99`, AUDIT-04 as `70a21db8`, and AUDIT-05 as
`2c0a3df9`. AUDIT-06 is committed as `53ecca2c` and AUDIT-07 as `b83f6243`.
All seven findings now have passing regressions. See the verification
record for checks and limitations.

An additional [independent step cancellation proof](isolated-step-cancellation-poc.md)
tests a possible architecture for stopping non-cooperative step code while its
parent continues. It uses guest-owned orchestration and one disposable Store per
step; it does not change this PR's production emitter or E128 behavior.
The follow-up [production research](isolated-step-production-research.md) measures
setup costs and package sizes and examines resource limits and durable races.
The [selective-isolation plan](selective-isolation-plan.md) maps the proposed
implementation to every existing DSL construct without changing the DSL.

[Open the interactive pattern guide](wasm-emitter-patterns.html) to compare tested
controls, recorded failures, and proposed fixes with step-through diagrams and
exportable example DSL. The guide is a standalone, offline HTML/CSS/JS page;
its traces illustrate the audit evidence and do not run WASM.

The original seven findings and subsequent follow-ups are documented below.
All **77 original audit tests pass**, with **none ignored**. There are also **42 passing unit tests**: 8 graph-analysis tests for
AUDIT-01, 6 arena tests for AUDIT-02, 7 identity tests and 1 compiler-version test
for AUDIT-03, 6 configuration tests and 1 compiler-version test for AUDIT-04,
2 timer-identity tests for AUDIT-05, 5 compiler tests, 2 backoff tests and 1 server
mapping test for AUDIT-06, plus 2 browser-validator tests and 1 server mapping test
for AUDIT-07.

The original known-defect tests assert the **desired correct behavior**. They
carried explicit ignore reasons while the defects remained open; all those ignores
have now been removed. Historical failure descriptions and verification records
below preserve the evidence from before each fix. This is a bounded audit, not a
complete proof of DSL correctness.

P1 denotes silent wrong execution or data loss; P2 denotes broken configuration,
deadline, or compiler/validation contracts. These findings do not establish Rust
memory-safety undefined behavior. AUDIT-01 fixes compiler graph analysis. AUDIT-02 changes arena collection and
adds two internal stdlib WIT functions; neither changes the authored DSL schema.

## Running the tests

The native suite lives in
[`tests/wasm_emitter_audit.rs`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs).
Execution cases live in
[`tests/wasm_emitter_audit/execution.rs`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs),
loaded as a module by the existing `direct_wasm_execute` harness. The harness is
reused without copying it or launching its socket-based helpers. Execution tests
pin the HostImport + InvokeHostImports ABI and use a checkpoint-preserving host
with a controlled clock. Temporary artifacts are removed after each test.

Run from the repository root with the pinned Rust toolchain and default compiler
feature. `RUSTC_WRAPPER=` was needed in the audit sandbox to bypass a blocked
sccache process; it is optional where the configured wrapper works.

```sh
# Only needed when shared components are missing or stale:
RUSTC_WRAPPER= RUNTARA_ONLY_WORKFLOW_COMPONENTS=1 RUNTARA_NO_INSTALL_TOOLS=1 scripts/build-agent-components.sh

# All audit controls and regressions:
RUSTC_WRAPPER= cargo test -p runtara-workflows --test wasm_emitter_audit
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit

# Focused identity validation, manifest and compiler tests:
RUSTC_WRAPPER= cargo test -p runtara-workflows --test wasm_emitter_audit audit_07

# One fixed finding, including controls and enabled regressions:
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit::audit_05 -- --include-ignored --nocapture
```

The compiler-only tests do not need prebuilt components. Execution tests need
current shared workflow components. The AUDIT-02 parallel case additionally needs
the Utils component and its metadata; run the build script without
`RUNTARA_ONLY_WORKFLOW_COMPONENTS=1` to build all agents. The audit tests need no
database, credentials, external services, or listening sockets. Do not omit the integration feature: Cargo would
otherwise skip the execution target.

<a id="audit-01"></a>

## AUDIT-01 · P1 — early Finish falls through to a falsely inferred merge

Reproduction graph (both conditions evaluate true):

```text
outer.true  -> inner
outer.false -> merge
inner.true  -> early Finish {result: "early"}
inner.false -> merge Finish {result: "merge"}
```

**Fixed and verified on 2026-09-06.** The graph validates, compiles, and now
returns `{result:"early"}` when both conditions are true. The other two routes
still return `{result:"merge"}`.

Before the fix, merge detection selected a node merely reachable from every
branch, even when a terminal path could avoid it. The dispatcher emitted that
continuation after the branch, overwriting the Finish output and potentially
running unrelated side effects.

The DSL support gate and manifest planner now share `common_post_dominator`:
a continuation is factored out only when **every path from every branch reaches
it**. A backwards worklist starts at each candidate and admits a predecessor
only when all its successors are guaranteed to reach the candidate. Separate
terminals, implicit exits, missing branches, and cycles that can bypass the
candidate prevent convergence. Candidate order preserves the nearest shared
continuation for genuine diamonds. Finish and Error have no successors within
their graph scope; `onError` handlers remain separate regions.

When there is no common continuation, the planner leaves the continuation in the
branches that actually reach it. Finish then ends its branch plan naturally.
No workflow-wide return is added: While/Split iteration outputs still flow back
to their loops, and embedded-child outputs still flow back to their parent.
Branch-specific continuations can produce more emitted code than a shared merge;
genuine diamonds retain their shared continuation.

Source: [shared merge analysis and unit tests](../crates/runtara-workflows/src/direct_wasm/graph_order.rs),
[manifest planner](../crates/runtara-workflows/src/direct_wasm/plan.rs),
[dispatcher](../crates/runtara-workflows/src/direct_wasm/compile/dispatcher.rs).

Invoke-ABI execution tests (all enabled and passing):

| Test in [`execution.rs`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs) | Contract verified |
| --- | --- |
| `audit_01_outer_false_reaches_merge` | Outer false returns merge output |
| `audit_01_inner_false_reaches_merge` | Nested false returns merge output |
| `audit_01_early_finish_terminates_before_merge` | Original regression returns early output |
| `audit_01_all_dispatch_combinations_preserve_selected_finish` | All 9 pairs of Conditional / routing Switch / conditioned-edge dispatch, with all 4 boolean combinations (36 executions) |
| `audit_01_early_finish_skips_durable_continuation` | For all 3 dispatch forms: early Finish neither suspends nor writes Delay checkpoints; continuing route suspends, then returns merge output on replay |
| `audit_01_implicit_finish_does_not_fall_through` | A terminal Log returns null without executing the merge, for all 3 dispatch forms |
| `audit_01_while_finish_exits_only_its_iteration` | Two iterations complete with early body output, then parent Finish runs |
| `audit_01_split_finish_exits_only_its_iteration` | Both sequential items yield early output, then parent continues |
| `audit_01_parallel_split_finish_exits_only_its_iteration` | Both parallel items yield early output, then parent continues |
| `audit_01_child_finish_returns_to_parent` | Embedded child yields early output to the parent Finish |

Eight unit tests cover nearest merges, bypassing terminals, nested diamonds,
duplicate targets, missing/disjoint branches, cycles, Finish/Error terminal
boundaries, and conditioned edges with separate error handlers. An independent
path-enumeration oracle also checks: all **1,024 forward DAGs on five
nodes** are checked against exhaustive terminal-path enumeration, verifying both
merge safety and whether a merge exists.

```sh
RUSTC_WRAPPER= cargo test -p runtara-workflows --lib graph_order::tests
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit::audit_01
```

<a id="audit-02"></a>

## AUDIT-02 · P1 — nested loop garbage collection deletes live outer values

**Fixed and verified on 2026-09-06.** A Filter's complete 100,000-character value
now survives a While containing another While. Before the fix this accepted
workflow completed with `{saved:null}`: inner mark/sweep saw only its own source
and state, so it deleted values still referenced by hidden outer frames.

The fix uses **allocation boundaries for scoped collection**:

- `value-store-scope` captures the arena's next allocation ID on entry to each
  While/Split. Both frame types save and restore the boundary alongside their
  existing heap watermark and loop state.
- `value-store-retain-scoped` preserves all entries older than that boundary,
  plus everything transitively reachable from the local source and survivor.
  It sweeps newer unreachable scratch on every sequential iteration or parallel
  chunk. It therefore protects outer frames without requiring their individual
  buffers to appear in a nested source.
- Once an inner loop exits, the restored enclosing boundary permits reclamation
  of its discarded values. No root registry needs cleanup on error/suspension.
  Boundaries are run-local, recreated on replay, and never checkpointed.
- Same-run dangling handles now fail with an explicit arena invariant panic
  (a guest failure when executing WASM), both during materialization and reference
  traversal. They cannot silently become a successful null or leak an internal
  handle. User-shaped and foreign-run handles remain ordinary data.

This is conservative: entries created before a loop starts remain protected for
its lifetime, including entries it does not itself reference. Memory still stays
bounded by those earlier allocations plus the local live state and scratch;
collection is not disabled for nested loops. Arena entries are immutable and IDs
monotonically allocated, which makes the allocation boundary safe.

The WIT additions are additive; `value-store-retain` remains for older callers.
**Rebuild the stdlib and recompile workflows to use the fix.** Existing composed
WASM artifacts retain their old emitter/stdlib code. Newly generated workflow
logic requires the new exports and will not compose with an older stdlib.

Source: [arena collection and invariant checks](../crates/runtara-workflow-stdlib/src/direct_json.rs),
[stdlib exports](../crates/runtara-workflow-stdlib/src/lib.rs),
[WIT contract](../crates/runtara-workflow-wit/wit/stdlib/runtara-workflow-stdlib.wit),
[While frames](../crates/runtara-workflows/src/direct_wasm/compile/while_loop.rs),
[Split frames/shared collector](../crates/runtara-workflows/src/direct_wasm/compile/split.rs),
[parallel chunk reset](../crates/runtara-workflows/src/direct_wasm/compile/split_parallel.rs).

Invoke-ABI tests in [`execution.rs`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs)
(all enabled and passing):

| Test | Contract verified |
| --- | --- |
| `audit_02_nested_loop_preserves_small_outer_value` | Original small-value control |
| `audit_02_single_loop_preserves_large_outer_value` | Original single-loop control |
| `audit_02_nested_loop_preserves_large_outer_value` | Original 100 KB regression; full contents survive |
| `audit_02_mixed_nested_loops_preserve_outer_source` | All four While/Split pairs and two alternating three-level combinations, with repeated iterations |
| `audit_02_nested_loop_preserves_value_near_intern_threshold` | 16,000 / 16,383 / 16,384 / 16,385 / 32,768-byte strings |
| `audit_02_child_loop_preserves_caller_source` | Embedded child with nested loops preserves caller state |
| `audit_02_nested_collection_survives_suspend_and_replay` | Repeated suspension and completion preserve the value; checkpoints contain no handles |
| `audit_02_nested_error_handler_preserves_outer_source` | Outer onError handler can still read the original large value |
| `audit_02_parallel_split_preserves_live_chunk_and_outer_values` | Real parallel lowering with four distinct 100 KB results across two chunks; nested While runs during assembly |
| `audit_02_nested_growing_accumulator_completes_with_bounded_memory` | Two outer passes each complete 60 inner iterations growing by 64 KiB, below a 64 MiB peak assertion under a 96 MiB per-memory cap |

Six stdlib unit tests additionally prove hidden outer-root retention, reclamation
of 100 generations of inner scratch (exactly two live entries and two dedup-index
entries after each collection), reclamation after restoring an outer boundary,
transitive survivor retention, malformed-root no-op behavior, and explicit
failures for dangling materialization and reference lookup. The compiler's
existing GC wiring test now checks calls to both new exports.

```sh
RUSTC_WRAPPER= RUNTARA_NO_INSTALL_TOOLS=1 scripts/build-agent-components.sh
RUSTC_WRAPPER= cargo test -p runtara-workflow-stdlib --lib audit_02
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit::audit_02
```

<a id="audit-03"></a>

## AUDIT-03 · P1 — sibling loop waits consume the same signal

**Fixed on 2026-09-06.** Two successive While bodies can now each contain a
locally named `wait` at index 0. The first response resumes only the first loop;
the second requests its own response. Replaying without a response preserves the
same address. Mixed While/Split nesting and repeated iterations use the same rule.

Before the fix, numeric ancestry omitted loop step identities. Both waits used
`.../wait/[0]`; delivering one response completed both loops. Agent and Split
checkpoints had the same omission. The new execution tests also check their
actual cached outputs (`A` and `B`) before and after replay, plus separate durable
Delay deadlines.

New artifacts use a structured key:

```text
runtara:v2:[operation, workflow, childNamespace, loopPath, operationFields]
loopPath = [["While", "a", 0], ["Split", "items", 2]]
```

Every loop frame records kind, local ID, and iteration. Child namespaces capture
the parent's loop path and invocation site before resetting the child's local
path. AI tool sites encode the AI step, label, and call counter as separate
fields. JSON encoding preserves delimiter-like IDs, quotes, and Unicode without
ambiguous concatenation. Child ancestry is a flat list, avoiding repeated
escaping of parent key strings. Large arena-backed identity values are resolved
before building keys. Authored loop variables and start inputs cannot replace
the compiler-owned version or loop path.

The shared identity applies to WaitForSignal, AI wait tools, Delay sleeps,
breakpoints, Agent and Split caches, embedded workflow caches, AI turn snapshots,
and embedded/composed child namespaces. Attempt/retry suffixes derive from the
complete structured base key. Configuration lookup uses the separate lexical
graph identity introduced by AUDIT-04 below.

**Compatibility:** manifest version 3 opts newly compiled workflows into key
version 2. Version 1/2 manifests retain the legacy key builders when compiled;
existing artifacts continue using their original compiled variables and keys.
Keep parked instances on their original artifact. Recompile workflows to obtain
the fix for future runs; do not replace a parked instance's artifact with a new
key version. There is no fallback from a v2 key to a legacy key, because doing so
would allow two new waits to consume the same old response. Signal senders should
use the opaque ID from the pending-input event/listing, rather than constructing
an address. An older composed child retains its internal legacy addressing and
must also be recompiled to fix collisions inside its own loops.

Source: [stdlib identity and key builders](../crates/runtara-workflow-stdlib/src/direct_json.rs),
[compiler version selection](../crates/runtara-workflows/src/direct_wasm/static_data.rs).

Execution tests in [the audit harness](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs):

| Test | Verified contract |
| --- | --- |
| `audit_03_distinct_wait_ids_suspend_independently_and_replay_stably` | Distinct local IDs and stable replay; legacy signals are not consumed |
| `audit_03_same_local_wait_id_suspends_independently` | Original collision regression; now enabled |
| `audit_03_sibling_mixed_loops_and_repeated_iterations_wait_independently` | All four While/Split sibling combinations, two iterations each |
| `audit_03_nested_sibling_loops_keep_the_complete_path` | Identically named inner loops under different outer loops |
| `audit_03_sibling_delays_checkpoint_independent_deadlines` | Two independent deadlines, unchanged on early replay |
| `audit_03_durable_agent_and_split_caches_keep_sibling_results_on_replay` | Different cached values survive fresh execution and replay |

[Seven stdlib identity tests](../crates/runtara-workflow-stdlib/src/direct_json_audit03_tests.rs)
cover all key builders, kind/site/index/ancestry separation, hostile delimiters,
tool fields, parent-to-child propagation, authored-variable overrides, large
interned paths/prefixes, and byte-exact legacy addresses. The compiler test
`audit_03_manifest_version_selects_compiler_owned_identity` pins version selection
and overrides authored identity defaults. Existing composed-child, AI tool,
breakpoint, retry, and delay tests assert the new key format and replay behavior.

```sh
RUSTC_WRAPPER= cargo test -p runtara-workflow-stdlib --lib audit_03
RUSTC_WRAPPER= cargo test -p runtara-workflows --lib audit_03
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit::audit_03
```

<a id="audit-04"></a>

## AUDIT-04 · P2 — repeated step IDs select another scope's runtime configuration

**Fixed on 2026-09-06.** Sibling loop bodies and embedded children can each
use a local `wait` with different settings. Both original regressions are enabled:
the second timeout resolves to 200 ms, rather than the first graph's 100 ms.

The previous registry flattened graph definitions into a map keyed by local step
ID, retaining the first match. Debug metadata independently searched flattened
configuration tables by the same ID. Durable invocation keys from AUDIT-03 could
separate responses while still selecting the wrong timeout, action, schema or
step type.

The registry now indexes definitions by **lexical graph path plus local ID**:

```text
root:                         []
While body:                   [["while.subgraph", "a"]]
Split inside that body:       [["while.subgraph", "a"], ["split.subgraph", "items"]]
Wait notification graph:      [["waitForSignal.onWait", "approval"]]
Preloaded embedded child:     [["embedWorkflow", "call"]]
lookup = registry[(graphPath, localStepId)]
```

The compiler initializes `_manifest_graph_path` to the root. Loop entry and
`onWait` append the defining role and owner; embedded calls select their preloaded
child graph. Returning restores the enclosing source. This path describes where
a step is **defined**, so it excludes iteration indices. AUDIT-03's durable path
continues to identify individual invocations. Authored start/iteration variables
cannot replace the private graph path; large interned paths are resolved before
lookup. A scoped lookup miss or malformed path errors instead of falling back to
a same-named step elsewhere.

Every definition also binds to the exact numeric mapping/condition/configuration
IDs from its own graph. Wait settings, breakpoint/debug metadata, and embedded
result/error envelopes select that definition. AI debug events select the main
agent record rather than a same-owner memory/tool record. Debug timers include
graph and invocation scope, so overlapping same-name spans do not overwrite one
another. Legacy aliases share the same definition allocation; configuration bodies
are not cloned into a second registry.

**Compatibility:** manifest version 4 opts newly compiled workflows into graph
paths; version 3's durable-key format is unchanged. Source without a graph path
retains legacy lookup. Additive WIT exports `wait-poll-interval-ms-scoped` and
`embed-workflow-error-scoped` carry the source required for selection; the old
exports remain available. Rebuild the stdlib and compiler together, then recompile
future workflow artifacts. Keep already parked instances on their original
artifacts. No authored DSL schema or stored-data migration is introduced.

The existing child-preload contract still requires unique EmbedWorkflow call-site
IDs across the preload bundle and rejects duplicates at the support gate. This fix
supports repeated **local IDs inside different child graphs**; it does not broaden
that separately enforced child-binding contract.

Source: [registry, scope propagation and metadata bindings](../crates/runtara-workflow-stdlib/src/direct_json.rs),
[compiler version selection](../crates/runtara-workflows/src/direct_wasm/static_data.rs),
[WIT exports](../crates/runtara-workflow-wit/wit/stdlib/runtara-workflow-stdlib.wit).

Native tests in [`wasm_emitter_audit.rs`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs)
(all enabled and passing):

| Test | Contract verified |
| --- | --- |
| `audit_04_distinct_loop_step_ids_keep_their_configuration` | Distinct loop-body IDs retain their timeouts |
| `audit_04_duplicate_loop_step_ids_keep_their_configuration` | Same loop-body IDs retain their timeouts; original regression |
| `audit_04_distinct_child_step_ids_keep_their_configuration` | Distinct child-local IDs retain their timeouts |
| `audit_04_duplicate_child_step_ids_keep_their_configuration` | Same child-local IDs retain their timeouts; original regression |

Invoke tests in [`execution.rs`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs)
(all enabled and passing):

| Test | Contract verified |
| --- | --- |
| `audit_04_sibling_loop_wait_settings_and_events_follow_their_graph` | All four While/Split sibling pairs: separate deadlines, poll settings, names, actions, correlation/context, response schemas and stable replay |
| `audit_04_embedded_wait_settings_and_events_follow_the_child_graph` | The same settings and replay assertions across two preloaded children |
| `audit_04_on_wait_graph_can_shadow_its_parent_step_type` | An onWait Finish can share its enclosing Wait's ID; debug output and parent resume remain correct |
| `audit_04_nested_step_type_and_debug_mapping_are_graph_local` | Root Finish and nested Filter share an ID while retaining their own types, mappings and outputs |

[Six stdlib unit tests](../crates/runtara-workflow-stdlib/src/direct_json_audit04_tests.rs)
cover strict scoped lookup and legacy fallback selection, authored-variable override
protection, Finish/breakpoint mapping selection, AI record binding, overlapping
debug spans, and an 80 KB interned path with delimiter/Unicode coverage. The compiler
test `audit_04_manifest_version_selects_compiler_owned_graph_path` pins the version
boundary and compiler ownership.

```sh
RUSTC_WRAPPER= cargo test -p runtara-workflow-stdlib --lib audit_04
RUSTC_WRAPPER= cargo test -p runtara-workflows --lib audit_04
RUSTC_WRAPPER= cargo test -p runtara-workflows --test wasm_emitter_audit audit_04
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit::audit_04
```

<a id="audit-05"></a>

## AUDIT-05 · P2 — While and Split timeouts restart after durable suspension

**Fixed on 2026-09-06.** A While or Split with a 1,000 ms budget containing
a 60,000 ms Delay now parks at the enclosing deadline, then returns
`WHILE_TIMEOUT` or `SPLIT_TIMEOUT`. Replaying early preserves the same deadline;
replaying late cannot grant a new budget. The two original regressions are enabled.

Before the fix, only a WASM local held `now + timeout`. Recreating the Store reset
that value, and the child's wake could postpone timeout handling by a full minute.
The iteration-count exit also preceded the timeout check, allowing final-body
overruns to complete successfully.

The compiler now maintains two records per completed timed-loop invocation:

- `loop-deadline`: the absolute deadline as eight little-endian bytes, created on
  first entry and restored on replay. A malformed deadline record fails with
  `LOOP_DEADLINE_STATE` instead of silently restarting the budget.
- `loop-complete`: a nonempty one-byte successful-exit marker. An empty checkpoint
  payload is a read-only probe in the runtime, so it cannot be used as this marker.
  Completed loops may still replay to reconstruct their outputs, but their old
  deadline no longer expires them after a later step suspends. This preserves
  existing result-cache behavior, including Split with `durable:false`.

Keys include operation, workflow, child ancestry, loop invocation path, loop type,
lexical defining graph and local ID. Timers therefore remain separate across
sibling loops, nested iterations and child calls. The stdlib's additive
`loop-deadline-key` export builds these keys and resolves interned ancestry.

Loop frames carry the earliest active enclosing deadline. Delay and retry `At`
wakes are clamped to this bound; signal waits keep their signal identity and gain
an earlier deadline when needed, including waits without their own timeout. The
child's own persisted deadline is unchanged. Normal returns and handled-error
unwinds restore the enclosing budget before a continuation or recovery handler
can suspend. Lifecycle pause/cancel remains an `OnResume` suspension, preserving
its existing semantics; elapsed wall time still counts when explicitly resumed.

Checks run before retry dispatch and at iteration boundaries, including the
final count-limit exit. Split's failed-attempt cache cannot bypass the deadline
check during replay. Expiry remains inclusive (`now >= deadline`). Zero or absent
timeouts continue to mean no timeout, as specified by the existing planner;
`now + timeout` saturates at `u64::MAX` instead of wrapping.

This is a cooperative wall-clock budget: a synchronous agent or blocking child
call is not preempted mid-call. Overrun is detected when control returns to the
loop boundary. While timeout errors retain their existing `onError` handling;
Split preserves its existing hard timeout failure, bypassing item aggregation,
retry and its own `onError` route. Changing that Split error-routing policy is
outside this deadline fix.

**Compatibility:** rebuild the shared stdlib and compiler together and recompile
future artifacts. The new compiler requires `loop-deadline-key`; existing artifacts
retain their old behavior. Keep parked instances on their original artifacts.
There is no checkpoint migration or authored DSL schema change. Timed loops now
write timer metadata even when result caching is disabled.

Source: [shared timer lowering](../crates/runtara-workflows/src/direct_wasm/compile/loop_deadline.rs),
[While boundaries](../crates/runtara-workflows/src/direct_wasm/compile/while_loop.rs),
[Split boundaries/retries](../crates/runtara-workflows/src/direct_wasm/compile/split.rs),
[wake emission](../crates/runtara-workflows/src/direct_wasm/compile/abi.rs),
[key construction](../crates/runtara-workflow-stdlib/src/direct_json.rs).

Invoke tests in [`execution.rs`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs)
(all enabled and passing):

| Test | Contract verified |
| --- | --- |
| `audit_05_while_completes_when_resume_is_within_timeout` | Original 500 ms control |
| `audit_05_split_completes_when_resume_is_within_timeout` | Original Split control |
| `audit_05_while_timeout_survives_suspend_resume` | Original late-replay regression |
| `audit_05_split_timeout_survives_suspend_resume` | Original Split regression |
| `audit_05_wakes_clamp_and_early_replay_keeps_the_original_deadline` | Wake at the loop deadline, unchanged checkpoints on early replay, expiry at equality |
| `audit_05_completed_loops_do_not_expire_during_later_replay` | A later Delay outlives an already completed loop; includes uncached Split results |
| `audit_05_final_body_overrun_fails_without_suspending` | Controlled clock advances at body Finish; final iteration fails without a park |
| `audit_05_signal_wakes_respect_the_enclosing_budget` | Waits with absent, shorter and longer timeouts; response inside the budget completes |
| `audit_05_nested_mixed_loops_keep_the_earliest_budget` | All four While/Split nesting pairs, with either inner or outer deadline earlier |
| `audit_05_sibling_loop_budgets_start_at_each_invocation` | Independent start times; expired completed sibling does not affect the next loop |
| `audit_05_invalid_deadline_checkpoint_fails_instead_of_resetting` | Malformed persisted bytes produce a structured error |
| `audit_05_zero_and_overflowing_timeouts_have_defined_boundaries` | Zero disables the budget; overflowing absolute deadlines saturate |
| `audit_05_split_retry_wait_does_not_extend_the_total_budget` | A 5-second retry backoff respects the 1-second whole-loop budget |
| `audit_05_embedded_child_wake_respects_parent_loop_deadline` | Child Delay wake intersects the parent's While/Split budget |
| `audit_05_handled_timeout_removes_the_expired_scope_before_recovery` | While onError recovery can park independently and replay to completion |
| `audit_05_child_timeout_unwind_restores_the_parent_budget` | A timed child fails into parent recovery without leaking its deadline |
| `audit_05_aggregated_inner_failure_does_not_leak_its_budget` | Split aggregation restores the active budget after an inner-loop error |

[Two stdlib unit tests](../crates/runtara-workflow-stdlib/src/direct_json_audit05_tests.rs)
check timer/completion separation, lexical and invocation identity, child namespaces,
Unicode/delimiters, non-loop rejection, replay stability and large interned paths.

```sh
RUSTC_WRAPPER= cargo test -p runtara-workflow-stdlib --lib audit_05
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit::audit_05
```

<a id="audit-06"></a>

## AUDIT-06 · P2 — maximum retry count overflows in the compiler

**Fixed on 2026-09-06.** `maxRetries` now accepts **0 through 4,294,967,294**
(`u32::MAX - 1`). The initial invocation counts as attempt 1, so at most
4,294,967,295 total attempts fit in the runtime's unsigned counter. Omitted retry
settings retain their existing defaults; this bound is a representation limit,
not a recommended operational budget.

The original Agent and EmbedWorkflow graphs with `maxRetries:4294967295` parsed
and passed the support gate, then panicked at `max_retries + 1` in debug builds.
The baseline rerun reproduced both failures. Split had the same unchecked
expression, and both AI Agent modes share Agent retry lowering. Original release
behavior was not executed; the fix is tested in both debug and release profiles.

The fix covers four step types and both AI modes:

- Agent and EmbedWorkflow use the top-level `maxRetries` field; Split and AI Agent
  use `config.maxRetries`. Save validation returns **E129 / RetryCountOverflow**,
  with the owning step and the `maxRetries` field in the server error DTO.
- The compiler support gate returns **retry-count-overflow**, even if callers skip
  save validation. Validation descends through Split/While subgraphs and
  WaitForSignal.onWait. Closure validation and compiler support also check
  preloaded children. No executable artifact is written for a rejected graph.
- A defensive manifest check precedes planning and returns **InvalidRetryBudget**.
  It covers agent records, Split config JSON, EmbedWorkflow body JSON, nested
  graphs and child graphs, including unsigned JSON counts wider than u32.
  Private lowering uses checked addition after this invariant is established.
- Backoff now uses saturating exponentiation before its existing saturating
  multiplication and delay cap. Previously `2u64.pow(attempt - 2)` could panic
  in debug or wrap to zero in release starting at attempt 66, even though the
  retry count itself was representable. Zero delay and retry-after overrides
  retain their existing behavior.
- Rate-limited retries retain their separate wait budget, so they may exceed
  `maxRetries`. All three emitted retry predicates now also require
  `attempt < u32::MAX`; even repeated zero-delay rate limits cannot wrap the
  attempt identity back to zero. Normal retry comparisons remain unsigned across
  the i32 sign boundary.

Source: [shared retry bound](../crates/runtara-workflows/src/retry_budget.rs),
[validation](../crates/runtara-workflows/src/validation.rs),
[support gate](../crates/runtara-workflows/src/direct_wasm/support.rs),
[manifest planning](../crates/runtara-workflows/src/direct_wasm/plan.rs),
[Agent retry lowering](../crates/runtara-workflows/src/direct_wasm/compile/agent_retry.rs),
[Embed retry lowering](../crates/runtara-workflows/src/direct_wasm/compile/embed_retry.rs),
[Split retry lowering](../crates/runtara-workflows/src/direct_wasm/compile/split_retry.rs),
[save-error mapping](../crates/runtara-server/src/api/dto/workflows.rs),
[shared retry backoff](../crates/runtara-workflow-stdlib/src/direct_json.rs) (`retry_delay_ms`).

Tests (all enabled):

| Test set | Confirmed behavior |
| --- | --- |
| `audit_06_{agent,embed,split,ai,ai_tool_loop}_retry_boundaries_below_overflow_compile` | Five tests each validate and compile omitted defaults, 0, 1, i32::MAX, i32::MAX + 1, u32::MAX − 2 and u32::MAX − 1; emitted components validate |
| `audit_06_{agent,embed,split,ai,ai_tool_loop}_retry_overflow_returns_compile_error` | Five tests assert E129 and structured compile rejection at u32::MAX, with no panic or executable artifact |
| `audit_06_nested_retry_overflow_is_rejected` | While, Split and onWait reject an overflowing nested Agent |
| `audit_06_child_retry_overflow_is_rejected` | Closure validation attributes the error to the child; compilation rejects the child's overflowing Split |
| `audit_06_planner_rejects_retry_overflow_without_support_gate` | Direct planning rejects modified AI records and a Split JSON count of u64::MAX |
| `audit_06_planner_checks_unreachable_nested_and_child_retry_budgets` | Manifest preflight checks unreachable EmbedWorkflow records inside nested and child graphs |
| `audit_06_{agent,embed,split}_emitted_retry_predicate_respects_unsigned_ceiling` | Three Wasmtime tests execute 45 production-predicate cases: zero/one retry, signed boundary, final attempt, retryable/permanent errors, exhausted wait budget and zero-delay rate limits |
| `audit_06_retry_backoff_saturates_across_the_unsigned_attempt_domain` | Delay remains capped from attempts 64–67 through u32::MAX; tests the exact u64 power boundary as well |
| `audit_06_retry_backoff_preserves_zero_override_and_attempt_caps` | Zero base/cap, retry-after overrides and total-attempt clamps retain their semantics |
| `retry_count_overflow_maps_to_a_stable_save_error` | Server DTO preserves E129, step identity, field name and the allowed/rejected counts |

Native fixtures: [wasm_emitter_audit.rs](../crates/runtara-workflows/tests/wasm_emitter_audit.rs).
Planner tests are in `plan.rs`; executable predicate tests are in
[retry_bounds_tests.rs](../crates/runtara-workflows/src/direct_wasm/compile/retry_bounds_tests.rs).

```sh
RUSTC_WRAPPER= cargo test -p runtara-workflows audit_06
RUSTC_WRAPPER= cargo test --release -p runtara-workflows audit_06
RUSTC_WRAPPER= cargo test -p runtara-workflow-stdlib --lib audit_06
RUSTC_WRAPPER= cargo test --release -p runtara-workflow-stdlib --lib audit_06
RUSTC_WRAPPER= cargo test -p runtara-server --lib retry_count_overflow_maps_to_a_stable_save_error
```

These tests establish counter arithmetic and rejection, not feasibility of
billions of external invocations. The predicate harness executes the real emitted
WASM at seeded counter values. Existing composed execution tests cover ordinary
Agent/AI/Embed/Split retry behavior. This change does not alter retry defaults,
backoff policy, the separate rate-limit budget, or existing artifact behavior.

<a id="audit-07"></a>

## AUDIT-07 · P2 — step map keys and inner IDs can disagree

**Fixed on 2026-09-06.** Every declaration must satisfy `steps[key].id == key`.
The comparison is exact: case, whitespace and Unicode are not normalized. Because
map keys are unique, this rule also guarantees unique inner IDs within a graph.
Independent nested and child graphs may still reuse the same local IDs.

The original root example
`entryPoint:"finish", steps:{finish:{id:"different",stepType:"Finish",...}}`
passed validation and the support gate, then compilation failed with
`missing direct entry step 'finish'`. A While-body mismatch and two map entries
sharing an inner ID also passed validation. The baseline run reproduced all three
failures at the intended rejection assertions; the matching control passed.

The fix adds a shared check used by validation, support analysis and manifest
construction:

- Validation returns **E130 / StepIdMismatch** for every mismatch, with
  `graph_path`, `step_key` and the conflicting `step_id`. It checks all 14 Step
  variants, including AI tool declarations, unreachable declarations and nested
  graphs beneath a missing parent entry point.
- Split and While bodies and WaitForSignal.onWait handlers are checked recursively.
  Paths are JSON pointers: the root is `""`, a While body might be
  `/steps/loop/subgraph`, and `/` and `~` in map keys are escaped as `~1` and `~0`.
  Diagnostic order is deterministic within each graph tree.
- Closure validation checks each preloaded child and attributes errors to its
  workflow ID/version. The support and manifest APIs also check every supplied
  child, even if the root does not call it; manifest construction includes those
  graphs. Their paths start with `/childWorkflows/{input_index}/executionGraph`.
- The support gate returns **step-id-mismatch** before running routing analysis,
  avoiding misleading routing errors. The public manifest builders independently
  return **DirectManifestError::StepIdMismatch**. Direct compilation propagates
  that structured error before creating an executable artifact, even when callers
  bypass save validation.
- The server DTO targets the authored map key and the `id` field. The browser
  validator receives the same E130 message and graph path. Valid graphs preserve
  their IDs and existing compilation behavior; no input is silently rewritten.

Correct an inconsistent declaration by making its inner `id` match the authored
map key. If renaming a step intentionally, keep its key, ID and references
consistent. This change validates the parsed DSL; it does not add identifier-format
restrictions or rewrite existing source definitions or compiled artifacts.

Source: [shared graph identity check](../crates/runtara-workflows/src/graph_identity.rs),
[validation](../crates/runtara-workflows/src/validation.rs),
[support gate](../crates/runtara-workflows/src/direct_wasm/support.rs),
[manifest construction](../crates/runtara-workflows/src/direct_wasm/manifest.rs),
[server error mapping](../crates/runtara-server/src/api/dto/workflows.rs),
[browser validation wrapper](../crates/runtara-validation-wasm/src/lib.rs).

The [14 native audit tests](../crates/runtara-workflows/tests/wasm_emitter_audit.rs)
are all enabled. Rejection helpers assert the exact structured fields, support
feature, manifest/compile errors and absence of an emitted executable:

| Test | Confirmed behavior |
| --- | --- |
| `audit_07_matching_step_keys_validate_and_compile` | Matching ASCII, punctuation and Unicode IDs validate and compile to valid WASM |
| `audit_07_root_key_id_mismatch_is_rejected` | Original root mismatch returns E130 and a structured compile error |
| `audit_07_nested_key_id_mismatch_is_rejected` | Original While-body mismatch reports its graph path |
| `audit_07_duplicate_inner_ids_are_rejected` | Original duplicate inner ID is rejected at the inconsistent map entry |
| `audit_07_split_and_on_wait_key_id_mismatches_are_rejected` | Both additional nested-graph forms are checked |
| `audit_07_nested_paths_escape_json_pointer_segments` | Deep mixed nesting preserves escaped `/` and `~` keys in the path |
| `audit_07_unreachable_and_invalid_entry_graphs_still_report_id_mismatches` | Unreachable steps and missing parent entry points cannot hide bad IDs |
| `audit_07_all_step_variants_check_their_declared_id` | All 14 current Step variants receive the same identity check |
| `audit_07_ai_tool_declarations_are_checked` | AI tool-edge targets are checked even outside normal flow |
| `audit_07_multiple_identity_errors_have_stable_order` | All mismatches are returned in stable path/key order |
| `audit_07_visually_similar_ids_are_not_silently_normalized` | Case, whitespace and composed/decomposed Unicode differences are rejected |
| `audit_07_local_ids_can_repeat_in_separate_nested_graphs` | Parent, While and Split graphs may each define their own `finish` |
| `audit_07_preloaded_child_identity_is_validated_before_manifest_construction` | Direct and nested child mismatches are rejected for referenced and unused supplied children |
| `audit_07_local_ids_can_repeat_in_separate_children` | Two child workflows may reuse the same local ID as the parent |

Two browser-wrapper tests confirm matching scoped IDs remain valid and root/onWait
mismatches expose E130. The server unit test
`step_id_mismatch_maps_to_the_authored_key_and_id_field` checks the UI anchor,
field name and diagnostic path.

```sh
RUSTC_WRAPPER= cargo test -p runtara-workflows --test wasm_emitter_audit audit_07
RUSTC_WRAPPER= cargo test -p runtara-validation-wasm audit_07
SQLX_OFFLINE=true RUSTC_WRAPPER= cargo test -p runtara-server --lib step_id_mismatch_maps_to_the_authored_key_and_id_field
```

## Verification record and remaining coverage

Original results on the audited implementation (before the AUDIT-01 fix):

| Check | Result |
| --- | --- |
| Native audit controls | 5 passed, 7 ignored |
| Invoke-ABI audit controls | 7 passed, 5 ignored |
| Native explicit defect run | 7 failed at the intended assertions/panic checks |
| Invoke-ABI explicit defect run | 5 failed at the intended assertions |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy -p runtara-workflows --all-targets --features direct-wasm-integration-tests -- -D warnings` | Passed (with `RUSTC_WRAPPER=`) |

The initial audit also ran 548 workflow library tests and 207 stdlib tests
successfully; one existing stdlib performance benchmark was ignored. Shared
components were rebuilt with the script above. The new tests extend the initial
reproductions to embedded-child registry collisions, Split deadlines, EmbedWorkflow
retry overflow, nested identity mismatches, and duplicate inner IDs.

Full CI, database/server E2E, all existing WASM execution tests, agent-component
rebuilds, and release-profile overflow execution were not run during the original audit.
The AUDIT-01 verification update below records the later checks.
Source review suggests broader exposure worth covering during fixes:

- AUDIT-03: production rollout with real persisted instances across artifact versions remains an operational integration check; local coverage now includes cached Agent/Split outputs, nested paths, version selection, and exact legacy addresses.
- AUDIT-04: local coverage now includes repeated IDs with different types, wait actions and response schemas; the existing global child-preload call-site uniqueness gate remains in force.
- AUDIT-05: wake clamping, final-body overrun and completed replay are now tested. Mid-call preemption and changing Split timeout error-routing policy remain outside this fix.
- AUDIT-06: debug/release retry counts and saturated backoff are now covered. Broader numeric-domain and operational stress testing remains outside this audit.

### AUDIT-01 verification update · 2026-09-06

- `cargo test -p runtara-workflows`: 584 passed across library/integration targets;
  7 unrelated known-defect audit regressions and 1 existing doctest ignored.
  The 556 library tests include all 8 new graph-analysis unit tests.
- AUDIT-01 invoke-ABI execution tests: 10 passed, none ignored. These compile the
  DSL, compose WASM, and execute it through the production component host with a
  controlled in-process runtime host; they cover the compiler-to-runtime boundary.
- Full `direct_wasm_execute` suite with `direct-wasm-integration-tests`: **188 passed,
  0 failed, 4 ignored** (the remaining AUDIT-02/03/05 defects). This includes the
  existing diamond, fan-out, error-routing, agent, and suspend/replay tests.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo clippy -p runtara-workflows --all-targets --features direct-wasm-integration-tests -- -D warnings`: passed.
- Interactive HTML: all **41 scenarios** passed JavaScript/DOM checks for family
  and mode navigation, variants, output, trace stepping/reset, DSL and test lists.
  Browser inspection confirmed the implemented-fix, historical-failure and
  supported-route views; the original early failure is no longer labeled an
  open gap. The HTML remains a standalone illustrative guide, not a WASM runner.

Commands used the pinned Rust 1.97.0 toolchain, `RUSTC_WRAPPER=`, the existing
Cargo target directory, and `RUNTARA_AGENT_COMPONENTS_DIR` pointing to the staged
shared/agent components in the original checkout. The first worktree invocation
reported missing components at the default worktree-relative path; rerunning with
the explicit component path passed. No guest component source changed. Database
and server APIs are outside this compiler fix; no database/server E2E was run.

### AUDIT-02 verification update · 2026-09-06

- Baseline reproduction before the fix: 2 controls passed and the original large
  nested-loop regression failed at the full-value assertion.
- Rebuilt all **27 agent components and both shared workflow components** with
  `scripts/build-agent-components.sh` into this worktree's own target directory.
- AUDIT-02 invoke-ABI tests: **10 passed, none ignored**.
- `cargo test -p runtara-workflow-stdlib --lib`: **213 passed**, 1 existing
  performance benchmark ignored; includes all 6 new arena unit tests.
- `cargo test -p runtara-workflows --lib`: **556 passed**.
- `cargo test -p runtara-component-host --features component-integration-tests --tests`:
  **45 passed** across library and component integration targets.
- Full `direct_wasm_execute` suite with the integration feature: **196 passed,
  0 failed, 3 ignored** (the remaining AUDIT-03/05 defects). AUDIT-01 stays green.
- Clippy for `runtara-workflow-stdlib`, `runtara-workflow-wit`, and
  `runtara-workflows`, all targets with the direct-WASM integration feature and
  `-D warnings`: passed. Formatting and `git diff --check`: passed.
- Interactive guide: **42 DOM scenarios** passed (navigation, variants, outputs,
  stepping/reset, DSL, test lists and fixed/proposed labels). Browser inspection
  confirmed the AUDIT-02 implemented-fix view and scoped-collection explanation.

The pinned Rust 1.97.0 toolchain was used with `RUSTC_WRAPPER=`. Host builds
reused the original checkout's Cargo target cache; WASM components were built and
loaded from the audit worktree to keep its new stdlib separate. Database/server
E2E and production deployment were not run; the changed boundary is DSL compilation,
component composition, guest arena collection, and invoke execution.

### AUDIT-03 verification update · 2026-09-06

- Committed the preceding AUDIT-02 change as `d787556e`; its pre-commit formatting
  and workspace Clippy checks passed. AUDIT-03 was subsequently committed as `a0629d99` before starting AUDIT-04.
- Before the fix, the distinct-ID control passed and the original same-ID wait
  regression failed: the second invoke completed instead of suspending.
- Rebuilt **27 agent components and both shared workflow components** into the
  audit worktree with `scripts/build-agent-components.sh`.
- AUDIT-03 invoke tests: **6 passed, none ignored**. Original same-ID regression
  enabled; the tests cover sibling/mixed/nested loops, repeat iterations, stale
  legacy signals, Delay deadlines, and actual Agent/Split cached outputs on replay.
- Identity unit tests: **7 passed**. Compiler version selection: **1 passed**.
- `cargo test -p runtara-workflow-stdlib --lib`: **220 passed**, 1 existing
  performance benchmark ignored.
- `cargo test -p runtara-workflows`: **557 library tests passed**, plus **28 native
  integration tests passed**; 7 remaining audit defects and 1 doctest ignored.
- `cargo test -p runtara-component-host --features component-integration-tests --tests`:
  **45 passed** across library and component integration targets.
- Full `direct_wasm_execute` suite with `direct-wasm-integration-tests`:
  **201 passed, 0 failed, 2 ignored** (the existing AUDIT-05 timeout regressions).
  This includes composed/embedded child signal replay, nested child namespaces,
  AI tool call scopes, AI turn replay, breakpoint resume, and retry isolation.
- Clippy for stdlib, workflow WIT, and workflows, all targets with the direct-WASM
  integration feature and `-D warnings`: passed. Formatting and diff checks passed.
- Interactive guide: **43 DOM scenarios passed**, including matching/distinct
  wait IDs, supported/historical/fixed views, DSL output, step navigation and
  reset. Browser inspection confirmed the layout, solution text, and independent
  second-wait trace.

Rust 1.97.0 and `RUSTC_WRAPPER=` were used. Host builds reused the original
checkout's Cargo target cache; guest components were built and loaded from this
worktree. Database/server E2E, a live migration of parked production instances,
and deployment were not run. Deploy the updated shared stdlib together with the
compiler, recompile future workflow artifacts, and retain the old artifacts for
already parked instances. No checkpoint or signal data migration is performed.

### AUDIT-04 verification update · 2026-09-06

- Committed AUDIT-03 as `a0629d99`; its pre-commit formatting and workspace
  Clippy checks passed. AUDIT-04 was subsequently committed as `70a21db8` before starting AUDIT-05.
- Before the fix, both distinct-ID controls passed and both duplicate-ID native
  regressions failed at the expected second-timeout assertion (100 instead of 200).
  After the fix, all **4 native AUDIT-04 tests pass**, with both ignores removed.
- Added **4 passing invoke tests** for sibling While/Split settings, embedded-child
  settings, onWait shadowing, and different step types/debug mappings. Added
  **6 passing stdlib unit tests** and **1 passing compiler-version test**.
- Rebuilt **27 agent components and both shared workflow components** using
  `scripts/build-agent-components.sh` in this worktree.
- `cargo test -p runtara-workflow-stdlib --lib`: **226 passed**, with 1 existing
  performance benchmark ignored.
- `cargo test -p runtara-workflows`: **558 library tests and 30 native integration
  tests passed**; 5 remaining audit defects and 1 existing doctest ignored.
- `cargo test -p runtara-component-host --features component-integration-tests --tests`:
  **45 passed** across library and component integration targets.
- Full `direct_wasm_execute` suite with `direct-wasm-integration-tests`: **205 passed,
  0 failed, 2 ignored** (the remaining AUDIT-05 timeout regressions). This includes
  existing AI wait tools, child error/retry paths, breakpoints, parallel loops,
  durable replay and bounded-memory tests.
- Clippy for stdlib, workflow WIT and workflows, all targets with the direct-WASM
  integration feature and `-D warnings`: passed. Formatting and diff checks passed.
- Interactive guide: **45 DOM scenarios passed**, including supported/historical/
  fixed views, same/distinct local IDs in loops and children, DSL output, trace
  navigation/reset, and links to both native and execution tests. Browser inspection
  confirmed the implemented-solution view and graph-qualified lookup explanation.
  Embedded example bundles now use the valid `latest` child-version selector.

Rust 1.97.0 and `RUSTC_WRAPPER=` were used. An initial shared-cache execution attempt
loaded runtime WIT 0.2 bindings from the main checkout into this WIT 0.1 worktree,
causing component composition to fail before execution. All final checks above
use this worktree's own host target directory and staged guest components. Use
separate Cargo target directories when these worktrees have different WIT inputs.
No database/server E2E, production artifact migration or deployment was run.


### AUDIT-05 verification update · 2026-09-06

- Committed AUDIT-04 as `70a21db8`; its pre-commit formatting and workspace
  Clippy checks passed. AUDIT-05 was subsequently committed as `2c0a3df9` before starting AUDIT-06.
- Before the fix, 2 controls passed and the 2 original timeout regressions failed:
  both resumed successfully after their original budget had expired.
- Rebuilt **27 agent components and both shared workflow components**, then
  refreshed the shared components after finalizing timer identity construction.
- Final AUDIT-05 invoke run: **17 passed, none ignored**. Both original regression
  ignores are removed. The final two cases verify embedded-child and aggregation
  error unwinds; the child-recovery fixture explicitly disables Embed retries.
- Timer identity unit tests: **2 passed**. Full stdlib library suite: **228 passed**,
  with 1 existing performance benchmark ignored.
- `cargo test -p runtara-workflows`: **558 library tests and 30 native integration
  tests passed**; 5 remaining AUDIT-06/07 defects and 1 existing doctest ignored.
- Component-host suite with `component-integration-tests`: **45 passed**.
- Full `direct_wasm_execute` run: **218 passed, 0 failed, 0 ignored**. The later
  final AUDIT-05 run includes the 2 additional unwind tests. Comparing the current
  target's test listing against both passing logs confirms **all 220 current
  execution tests are covered** across those runs. No WASM audit regression is
  still ignored. Existing AI, retry, parallel, replay and memory tests remain green.
- Final Clippy for stdlib, workflow WIT and workflows, all targets with the
  direct-WASM integration feature and `-D warnings`: passed. Formatting and
  `git diff --check` passed.
- Interactive guide: **47 DOM scenarios passed**, checking supported/historical/
  fixed views, timeout DSL variants, expected outputs, trace navigation/reset and
  links to all 17 execution cases. Browser inspection confirmed the updated
  deadline diagram and implemented-solution explanation.

Checks used Rust 1.97.0, `RUSTC_WRAPPER=`, and this worktree's own host build cache
and guest components. No database/server E2E, production artifact migration or
deployment was run. The fix preserves cooperative execution and the existing
Split timeout error-routing policy described above.

### AUDIT-06 verification update · 2026-09-06

- Committed AUDIT-05 as `2c0a3df9`; its pre-commit formatting and workspace
  Clippy checks passed. AUDIT-06 was subsequently committed as `53ecca2c` before starting AUDIT-07.
- Baseline retry-count run: **2 controls passed, 2 regressions failed** at the
  intended panic assertions. The original ignores are now removed.
- Debug and release AUDIT-06 runs: **12 native tests and 5 compiler unit tests
  passed in each profile**. The latter include 45 actual Wasmtime executions of
  the Agent/Embed/Split retry predicates and defensive manifest-planner checks.
- Added backoff tests first: **both failed** with multiplication overflow.
  After saturating exponentiation, **both pass in debug and release**. The full
  stdlib library suite passes **230 tests**, with 1 existing benchmark ignored.
- Server save-error mapping test: **1 passed**. Browser validation builds with
  `cargo check -p runtara-validation-wasm --target wasm32-unknown-unknown`, which
  also checks that E129 works without the compiler feature.
- Refreshed **27 agent components and both shared workflow components** using
  `scripts/build-agent-components.sh`. Component-host tests with the
  `component-integration-tests` feature: **45 passed**, none ignored.
- Final `cargo test -p runtara-workflows --features direct-wasm-integration-tests`
  after the guest rebuild: **565 library tests, 220 composed execution tests and
  41 native integration tests passed**. The only ignored audit regressions are
  the 3 remaining AUDIT-07 cases; 1 existing doctest is also ignored. All 47
  invoke-ABI audit cases and all 17 enabled native audit cases pass.
- Explicitly reran the 3 ignored AUDIT-07 cases: all still fail at the intended
  key/inner-ID validation assertion. They remain documented open defects.
- Clippy for workflows, stdlib and server, all targets with
  `runtara-workflows/direct-wasm-integration-tests` and `-D warnings`: passed.
  Formatting and `git diff --check`: passed.
- Interactive guide: **56 DOM scenarios passed**, including all five retry
  shapes, exported DSL counts and AI tool edges, test links, detail navigation,
  trace stepping/reset, and historical/fixed labels. Browser inspection confirmed
  the fixed Split and AI tool-loop views and corrected an overflowing graph label.

Checks used Rust 1.97.0, `RUSTC_WRAPPER=`, `SQLX_OFFLINE=true` for the server checks,
and the audit worktree's own host cache and guest components. No database/server
E2E, deployment, production artifact migration, or billions-of-retries stress run
was performed. The new compiler contract applies to newly built artifacts; the
backoff change requires rebuilding the shared stdlib and recomposing workflows.

### AUDIT-07 verification update · 2026-09-06

- Committed AUDIT-06 as `53ecca2c`; its pre-commit formatting and workspace
  Clippy checks passed. AUDIT-07 was subsequently committed as `b83f6243` with
  the same checks passing.
- Baseline: **1 control passed and 3 regressions failed** at their intended
  identity-rejection assertions. All three ignores have now been removed.
- AUDIT-07 native tests: **14 passed**, covering all 14 step variants, every
  nested graph form, referenced/unused children with direct/nested mismatches,
  exact Unicode/case/whitespace equality, escaped diagnostic paths, deterministic
  error ordering and legitimate local-ID reuse. Successful controls also validate
  the emitted WASM; failure cases assert that no executable was emitted.
- Full `cargo test -p runtara-workflows --features direct-wasm-integration-tests`:
  **565 library tests, 220 composed execution tests and 54 native integration
  tests passed**. All **77 audit cases** pass; none are ignored. One existing
  documentation example remains ignored.
- `cargo test -p runtara-validation-wasm`: **22 passed**, including both new E130
  wrapper tests. These execute the wrapper's Rust implementation natively.
  `cargo check -p runtara-validation-wasm --target wasm32-unknown-unknown` passed
  with the compiler feature disabled.
- Server error-mapping test: **1 passed**, asserting E130, the authored step key,
  the `id` field and the nested graph path.
- Clippy for workflows, server and browser validation, all targets with
  `runtara-workflows/direct-wasm-integration-tests` and `-D warnings`: passed.
  Formatting and `git diff --check`: passed.
- Interactive guide: **66 DOM scenarios passed**. Identity cases check exported
  graphs and child bundles, AI tool edges, exact mismatch counts, graph paths,
  fixed/historical labels, expected outcomes, test links and trace navigation.
  Browser inspection confirmed the child diagnostic and diagram layout.

Checks used Rust 1.97.0, `RUSTC_WRAPPER=`, `SQLX_OFFLINE=true` for server checks,
and the audit worktree's own host build cache and staged guest components from
AUDIT-06. No WIT or guest source changed, so components were reused. No database
E2E, deployment, or migration of existing definitions/artifacts was performed.

### Upstream integration verification · 2026-09-06

Merged upstream `main` at `6f7db0c4a6539bc288f0e114fdf27331e3674ae1`
into the audit branch. The merge includes upstream's workflow runtime WIT
`0.3.0` and command-identity handling. Rebuilt all **27 agent components and
2 shared workflow components** in the audit worktree before execution checks.

- Full workflows suite with `direct-wasm-integration-tests`: **565 library,
  221 composed execution, and 54 native integration tests passed**. This includes
  all **77 audit cases** and upstream's command-identity execution regression.
  One existing doctest remains ignored.
- Stdlib suite: **230 unit tests and 1 doctest passed**; one existing performance
  benchmark remains ignored.
- Component host with `component-integration-tests --tests`: **45 passed**,
  including the full-bundle dispatcher drift detector.
- Browser validator: **22 passed**; its `wasm32-unknown-unknown` check passed.
- Server workflow DTO tests: **25 passed**, including E128, E129 and E130 mappings.
- Clippy for workflows, server and browser validation, all targets with the
  workflow integration feature and `-D warnings`: passed. Formatting and
  `git diff --check`: passed.
- Interactive guide: all **66 DOM scenarios passed** again.

Checks used pinned Rust 1.97.0, the worktree's own component/build caches,
`RUSTC_WRAPPER=`, and `SQLX_OFFLINE=true` for server checks. The complete CI
service matrix, database E2E, deployment and production artifact migration were
not run locally. Earlier verification records above describe their original
bases; this section records the combined result after integrating upstream.


<a id="audit-08"></a>

### AUDIT-08 · Retry budget inconsistencies found during nested cancellation

**Observed existing behavior; not changed by the cancellation implementation.**

| Case | Current behavior | Test |
| --- | --- | --- |
| Recognized `SLACK_RATE_LIMITED` with `maxRetries: 0` | The emitter bypasses the entire retry loop, so the first error follows onError even with unused rate-limit budget | `nested_retry_zero_retries_routes_rate_limit_error_to_recovery` |
| HTTP agent returns `HTTP_429` with `maxRetries: 1` | Two errors exhaust ordinary retries and follow onError; the classifier recognizes `RATE_LIMITED`, not `HTTP_429` | `nested_retry_http_429_uses_ordinary_retry_count` |
| Recognized rate-limit error with `maxRetries: 1` | Separate rate-limit budget permits two waits and a successful third call | `nested_retry_preserves_rate_limit_budget_beyond_ordinary_retry_count` |

The tests run actual built HTTP/Slack agents inside two composed workflow-agent
levels, check request counts and the final success/recovery output, and preserve
the current behavior. Source: `compile/agent.rs` gates the retry loop on
`max_retries > 0`; `runtara-workflow-stdlib/src/direct_json.rs` classifies codes
containing `RATE_LIMITED`. The HTTP agent emits `HTTP_429`.

A possible correction is to define rate-limit classification consistently and
explicitly decide whether setting zero ordinary retries should leave the
separate rate-limit budget active. That would change accepted execution behavior
and needs matching sequential/parallel, replay and provider tests. Merely
removing the loop guard is insufficient: parallel eligibility and attempt
checkpoint identity depend on the retry path too.

Tests live in
[`nested_retry.rs`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/nested_retry.rs).


<a id="audit-09"></a>

### AUDIT-09 · Loop cancellation and runtime-free While emission

**Fixed in the cooperative cancellation worktree, 2026-09-07.**

| Case | Before | Current behavior / regression test |
| --- | --- | --- |
| Non-durable While published as an agent | Static analysis allowed runtime omission, but emitted heartbeat/cancellation calls referenced a missing runtime index and failed validation | `published_while_without_runtime_preserves_completion` checks compilation, execution and exact iteration/output results without the root runtime import |
| CPU-only published Split | No cancellable wait between items; parent cancellation remained unresolved until the whole-run watchdog | `published_split_yields_for_parent_cancellation_between_items` checks standard cooperative completion after a completed warmup request |
| Root While/Split with no Agent wait | Pure-loop boundaries did not use the new non-consuming signal path | `root_{while,split}_observes_cancel_without_an_agent_wait` requires the root receipt and no success publication |
| While beside a pending HTTP branch | Direct consuming cancellation checks could bypass shared sibling cleanup | `emitted_while_boundary_cancel_cleans_pending_sibling_before_ack` rejects early consuming checks and requires socket closure before acknowledgement |
| Signal-check error after adding a guard | An unadjusted WASM branch depth could change While recovery | `while_boundary_legacy_{cancel,check}_error_keeps_on_error_routing` checks the final recovery output |

The shared iteration boundary uses standard cancellable `thread.yield` in
workflow-agents and existing lifecycle polling/cleanup in roots. It introduces
no authored flag or host task API. Additional tests cover While body cancellation,
two inline Embed scopes, partial body cleanup and parallel/While child shapes.
Long operations inside one iteration still need their own cooperation points;
these tests do not certify arbitrary native or Agent CPU code.

Execution tests:
[`loop_boundaries.rs`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/loop_boundaries.rs)
and the existing
[`cooperative workflow suite`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/mod.rs).
The standard yield proof is in
[`cooperative_cancellation.rs`](../crates/runtara-component-host/tests/cooperative_cancellation.rs).


<a id="audit-10"></a>

### AUDIT-10 · Root cancellation during non-durable Agent retry backoff

**Fixed in the cooperative cancellation worktree, 2026-09-07.**

| Case | Before | Current behavior / regression test |
| --- | --- | --- |
| Cancel during ordinary root Agent backoff | Blocking runtime sleep prevented guest polling until the delay ended; the five-second watchdog expired for a 60-second delay | `root_retry_cancel_interrupts_long_backoff` requires cooperative suspension and acknowledgement after exactly one request, with no retry/recovery |
| Cancel during recognized rate-limit backoff | Same uninterruptible guest wait despite a pending lifecycle command | `root_retry_cancel_interrupts_rate_limit_backoff` checks the same cancellation outcome through a real Slack error envelope |
| Ordinary backoff with no cancellation | Wait, retry and recover/succeed according to existing policy | `root_retry_preserves_success_and_backoff_after_transient_errors` checks three requests, exact output and elapsed delay |
| Rate-limit budget beyond ordinary retries | Separate recognized rate-limit budget may permit additional attempts | `root_retry_preserves_rate_limit_budget_beyond_ordinary_retry_count` checks the third request succeeds |
| Zero ordinary retries and HTTP_429 | Existing AUDIT-08 policy discrepancies | `root_retry_zero_retries_routes_rate_limit_error_to_recovery` and `root_retry_http_429_uses_ordinary_retry_count` retain the existing request counts and recovery result |

The fix routes non-durable Agent backoff through the existing async timer and
shared guest wait/cleanup. Durable lifecycle retries still park. Tests share
[`nested_retry.rs`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/nested_retry.rs)
with the published workflow-agent cases. Legacy/capability WaitForSignal polling
and Embed/Split blocking backoff remain separate gaps; production root
WaitForSignal already parks on a signal. The lower-level non-durable Delay
emitter blocks, but production rejects that graph to avoid holding a runner.
This finding does not change that acceptance boundary, establish those paths'
cancellation behavior or implement per-step timeout support.


<a id="audit-11"></a>

### AUDIT-11 · Composite retry waits without Agent imports

**Fixed in the cooperative cancellation worktree, 2026-09-07.**

| Case | Before | Current behavior / tests |
| --- | --- | --- |
| Cancel during non-durable Embed/Split backoff | Blocking sleep prevented guest polling; a 60-second backoff reached the five-second watchdog | `agent_free_embed_backoff_cancels_without_recovery`, `agent_free_split_backoff_cancels_without_recovery` require suspension and acknowledgement after one child attempt |
| No cancellation | Retry, then follow onError after exhaustion | `agent_free_*_backoff_preserves_attempts_and_recovery` check three Error events, elapsed delay and exact recovery output |
| Zero delay or zero retries | Immediate wait or direct error routing | `agent_free_*_zero_backoff_preserves_attempts`, `agent_free_*_zero_retries_recovers_immediately` retain counts/results; zero retries requires no timer import |
| Retry inside While or an inline child | Timer need exists below the root graph | `nested_while_*_backoff_cancels`, `inline_child_*_backoff_cancels_with_outer_retries_disabled` prove discovery and cancellation through the nested scope |
| Agent error reaches a retrying Split | Enclosing backoff blocks after the HTTP response | `split_retry_after_http_error_cancels`, `split_retry_after_rate_limit_error_cancels` stop after one request; `split_retry_after_http_errors_preserves_success` retains normal success |
| Scaffold regenerated for explicit runtime binding | A new timer requirement could be omitted from the regenerated WIT | Every Agent-free case checks timer presence/absence and equality of returned and on-disk WIT |

The fix uses one shared async timer wait for non-durable Agent/Embed/Split retries.
Required imports and helper functions are provisioned even without an Agent;
durable lifecycle retries still park. Tests live in
[`composite_retry.rs`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/composite_retry.rs)
and [`nested_retry.rs`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/nested_retry.rs).
Callable composite publication, per-step deadlines and full resource/capacity
qualification remain open.


<a id="audit-12"></a>

### AUDIT-12 · Formatted Agent errors lose the composite error contract

**Original finding (the timer migration retained this behavior).**

`DirectJsonManifest::agent_error` and `agent_error_from_info` produce
`Step … failed: Agent …: {…}` text. `embed_workflow_error_scoped` expects JSON,
while `workflow_retry_info` falls back to an ordinary retryable error when JSON
parsing fails.

| Case | Original behavior before correction | Regression tests (now assert corrected behavior) |
| --- | --- | --- |
| HTTP or Slack Agent fails inside Embed | The child error cannot be parsed; execution fails before Embed enters backoff or its onError recovery | `embed_retry_after_http_errors_preserves_success`, `embed_retry_preserves_rate_limit_budget_beyond_ordinary_retry_count` |
| Fixture schedules cancellation after that error response | Existing parse failure prevents entering backoff; no cooperative acknowledgement is reported. This does not qualify post-terminal signal handling | `embed_retry_after_http_error_cancels`, `embed_retry_after_rate_limit_error_cancels` |
| Recognized Slack rate-limit error reaches a retrying Split with `maxRetries: 1` | Formatted text loses classification, two requests exhaust ordinary retries and recovery runs despite unused rate-limit budget | `split_retry_preserves_rate_limit_budget_beyond_ordinary_retry_count` |

The no-cancel Embed and Split cases also failed their intended-success assertions
with the original blocking composite sleep calls restored temporarily, confirming
that the new timer did not introduce these behaviors. The final compatibility
tests originally asserted those outcomes; the correction below replaces those assertions.

A possible correction is a shared structured error representation across Agent
and composite propagation, with human-readable formatting confined to presentation.
Qualify code/category/retryability/retry-after preservation, nested wrapping,
onError payloads and durable attempt replay before changing this contract. Parsing
an arbitrary substring of a formatted message would be ambiguous and fragile.
These tests live in
[`nested_retry.rs`](../crates/runtara-workflows/tests/cooperative_workflow_cancellation/nested_retry.rs).

**Correction · 2026-09-08.** Shared Agent failure conversion
now emits a JSON object; Embed propagates the originating error code and retry
fields, and composite retry honors explicit nonretryability. The original parse
failure assertions are replaced by real retry/cancellation outcomes. Added tests
cover permanent errors reaching onError without retries and durable Embed replay
without another failed HTTP request. Root Agent failure exports now have typed
fields, and Embed no longer always masks the originating code with
`CHILD_WORKFLOW_FAILED`. See the implementation record for compatibility scope
and final validation. E128 and the separate AUDIT-08 policy gap remain unchanged.

### AUDIT-11 publication update · 2026-09-07

Non-durable Split retry waits can now be published inside workflow agents when
the complete closure has no root runtime requirement. The shared standard timer
wait propagates parent cancellation through two composed workflow agents. Nine
new `published_split_retry_*` tests in
`tests/cooperative_workflow_cancellation/nested_retry.rs` cover cancellation,
normal retries, recovery, zero retries and existing error classification. They
also verify the established sequential fallback when Split-level retries are
combined with requested parallelism. These cases do not certify concurrent retries
or repair AUDIT-08/AUDIT-12. Embed retry publication remains gated.

The safety check also exposed missing Split timeout feature metadata; the feature
walk now records it at every nesting level. The publication regression test
`workflow_agent_safety_accepts_split_backoff_only_without_root_runtime_ownership`
checks durability, event/logging/error paths, signals, breakpoints and nested
Split timeout rejection. This is import/safety analysis, not Agent/Embed timeout
implementation; E128 remains in force.

### Agent/Embed timeout follow-up · deadline race contract · 2026-09-08

E128 remains in force. A new standard-component deadline fixture supplies
executable rules for the missing timeout path: completion wins when completion
and expiry are observed ready together; once expiry is selected, a normal value
returned during cancellation cleanup is too late to replace it. Cleanup precedes
continuation and target component reuse; the sibling test operation survives.

`runtara-component-host/tests/cooperative_cancellation/deadline.rs` contains six
built-HTTP cases, a synthetic return-during-cleanup case and two negative mutation
checks. These prove the primitive selection/cleanup contract, not emitted Agent
or Embed deadline semantics. Authored zero/overflow behavior, durable budget
restoration, inheritance, retries/recovery and emergency grace still require
integration evidence. See the cooperative cancellation implementation record.


### AUDIT-11/12 follow-up · published and nested Embed · 2026-09-08

Published Embed retries now keep their waits in the guest when every supplied
child is non-durable and runtime-free. The common completion boundary yields to
parent cancellation instead of reading the root signal. Closure tests cover
runtime-requiring children at root, child and grandchild depth and validate both
retained-runtime and omitted-runtime component shapes.

Nested error tests also found that an inner Embed could overwrite the outer
attempt's parent-source locals. Wrapping the outer error then failed with an
unknown-step lookup in the inner graph. The shared attempt frame now restores
parent source, child input, saved data and retry-key locals before error handling
and retries. This repair also applies to top-level nested Embed execution.

| Case | Corrected behavior | Test |
| --- | --- | --- |
| Published Embed waits after HTTP/Slack failure | Parent cancellation stops backoff without another request or recovery | `published_embed_retry_after_http_error_cancels`, `published_embed_retry_after_rate_limit_error_cancels` |
| Two inline Embeds inside two published workflow components | Errors reach the correct outer recovery/retry scope | `published_nested_embed_retry_preserves_rate_limit_budget`, `published_nested_embed_permanent_error_preserves_recovery_fields` |
| Nested HTTP input mapping followed by retry | URL survives both child scopes and reaches subsequent attempts | `published_nested_embed_retry_restores_input_after_http_error`, `root_nested_embed_retry_restores_input_after_http_error` |
| Published Embed child contains only Finish/mapping | Default/zero/nonzero retry settings preserve normal output without lifecycle runtime imports in the child | `published_embed_pure_child_preserves_output_without_runtime_or_agent_io` |

Execution cases are in `cooperative_workflow_cancellation/nested_retry.rs`.
Compiler and safety tests cover runtime ownership and import validity. Pure-child
normal execution does not prove cancellation inside a failing Agent-free
callable retry. E128 and the AUDIT-08 retry-policy differences remain unchanged.

### AUDIT-12 follow-up: plain computation errors (2026-09-08)

The published Agent-free retry qualification exposed another child-error contract
gap: stdlib computation helpers return plain WIT error strings. Embed's shared
error wrapper demanded JSON, so an integer-coercion failure became a parse error
and bypassed that Embed's retry/recovery. The equivalent Split cases passed.
This was reproduced in both root recovery and published cancellation tests before
the fix.

Both Embed wrappers now preserve plain errors under `childError` and retain the
existing generic code/transient retry policy. Structured payloads keep their
fields; JSON fragments inside plain text cannot override retry policy. Native
unit tests cover scoped/unscoped and nested wrapping, empty/invalid UTF-8 text,
JSON scalar/array values, and structured errors. The executable regressions are
in `crates/runtara-workflows/tests/cooperative_workflow_cancellation/pure_retry.rs`:

- `published_pure_embed_backoff_cancels`
- `published_pure_split_backoff_cancels`
- `pure_embed_errors_preserve_retries_and_recovery`
- `pure_split_errors_preserve_retries_and_recovery`

These tests use no Agent in the failing graph and cover root/published recovery,
zero retries/delay, actual delayed retries, and cancellation through two published
components. See the implementation record for timing assumptions and verification
scope. E128 remains in force; this does not release scoped Agent/Embed timeouts.

### Scoped deadline lowering prerequisite (2026-09-08)

The shared Await emitter now includes an owned deadline timer input and a scoped
timeout outcome. `compile/cooperative_wait_tests.rs` executes the generated core
helper to check completion/deadline ordering, already-due deadlines, cleanup that
returns a late value, timer resolution, root/parent cancellation, and an unrelated
sibling remaining live after timeout. Its deterministic canonical-event fixtures
complement the separate real-component deadline tests; they are not DSL timeout
execution tests.

Current DSL call sites do not create that deadline input or consume its timeout
outcome. **Agent/Embed timeouts remain rejected with E128, and existing loop
budgets still require integration to interrupt pending I/O.** Remaining work must
preserve scope ownership, inherited/durable budgets, recovery context, sibling
execution and cleanup grace. See the implementation record for exact evidence
and limits; the emitter primitive alone does not close the timeout audit gap.


### Agent deadline lowering qualification · 2026-09-08

The emitter now retains an authored Agent timeout and integrates the sequential
invocation/retry budget with standard subtask cancellation. Public E128 rejection
is unchanged. Private-emitter composed tests exercise HTTP cleanup followed by
structured recovery, no automatic retry after timeout, zero and u64::MAX,
backoff clipping, early/expired durable replay, result-cache replay, root Cancel
bypassing recovery, later-attempt budget preservation, and corrupt budget state.
Tests: `crates/runtara-workflows/src/direct_wasm/compile/agent_deadline_tests.rs`
(existing `direct-wasm-integration-tests` test feature; built components required)
and stdlib `agent_deadline_identity_is_distinct_and_stable_across_attempts`.

This does not close timeout support: the owning budget starts after input
mapping/validation/cache lookup, uses the runtime wall clock, and does not yet
interrupt connection preparation, carry inherited expiry through nested recovery,
or provide parallel scope ownership and cleanup grace. Published runtime-free
clock lowering, monotonic clock behavior, Embed/AI scope coverage, and the wider
qualification gates remain open. No new public supported-pattern claim is made.


### Standard-clock deadline qualification · 2026-09-08

Agent live budgets now use standard WASI monotonic elapsed time. Non-durable
published workflows enforce their own budget without a lifecycle runtime import;
durable scopes reconstruct the live duration from their persisted epoch deadline.
Forward/backward fixture epoch-clock changes cannot alter an active budget.
Published one/two-layer timeout/cancel/retry tests and emitted arithmetic boundary
tests cover this path. The Agent deadline inventory now includes inline nested
definitions, verified through one/two While levels with timeout and ordinary
success/error controls. Tests remain in `compile/agent_deadline_tests.rs`, with
arithmetic coverage in `compile/agent_deadline.rs`.

E128 is unchanged. This does not qualify inherited expiry ownership, recovery
that continues an enclosing loop, scoped parallel cancellation, preparation,
cleanup grace, or arbitrary wall-clock correction while parked. Full current
coverage and verification are recorded in the implementation document.


### Inherited loop deadline qualification · 2026-09-08

The current worktree selects the earliest enclosing While/Split budget during
sequential child I/O and at loop boundaries, using the standard monotonic clock
for live elapsed time. A guest-local owner and unwind reason prevent a child
handler, retry, or Split aggregation from consuming its parent's expiry. The
owning loop restores its parent scope before recovery. Durable epoch records,
completed-loop markers and parked-time semantics remain intact; retry wakes are
now clamped before checkpointing too.

New composed tests exercise nested scope precedence, ordinary controls, root
Cancel, backoff/replay, CPU-loop interruption with a frozen epoch, recovery after
expiry, AI provider/memory I/O and two nested Embed levels. AUDIT-05 now separates
real final-body overrun from a wall-clock jump and retains its mixed-loop,
aggregation, wake-clamp and replay checks. The patterns page reflects the current
clock model. See `cooperative-cancellation-implementation.md` for exact evidence.

This is partial G5/G6/G7 qualification. Parallel scope timers, own Embed/AI/tool
budgets, preparation waits, bounded cleanup grace, E2E, soak and controlled size/
timing comparisons remain required. Public Agent/Embed timeout syntax still
returns E128. Existing loop zero-timeout behavior remains disabled.


### Parallel deadline qualification · 2026-09-08

Enclosing While/Split deadlines now interrupt pending parallel windows through
the standard shared guest wait. A Split's own timeout retains parallel I/O for
otherwise eligible bodies. Deterministic emitted-helper tests verify ready-event
ties, timer/call separation, cancellation resolution and balanced pause deferral.
Production HTTP tests require two live calls, cleanup before recovery, ordered
results despite reverse completion, ordinary error routing, and durable replay.
A fast-branch overrun test also forbids its next HTTP request while requiring
cleanup of the pending peer. Tests are in `compile/cooperative_wait_tests.rs` and
`tests/cooperative_workflow_cancellation/{mod.rs,parallel_deadline.rs}`.

Individual timed-Agent sibling preservation, overlapping nested scopes,
interruptible preparation, own Embed/AI/tool deadlines and cleanup grace are not
fully qualified. Agent/Embed timeout syntax remains E128-gated. The implementation
record lists final verification; this stage adds no controlled benchmark claim.

### Connection preparation qualification · 2026-09-08

The shared compiler path now awaits connection metadata through the standard
Component Model cancellation helper. A hung lookup can be interrupted before
Agent invocation, including metadata headers and partial response bodies.
Sequential, AI, inline Embed, published-child, Split and branch paths use this
same helper. Inherited expiry resolves pending peers and returns to the owning
scope instead of invoking the Agent or entering its retry/error path.

Tests live in `tests/cooperative_workflow_cancellation/preparation.rs` and
`compile/agent_deadline_tests.rs`; resolver ABI/cache compatibility is covered in
`runtara-component-host/src/workflow/connection_resolver_tests.rs`. The resolver's
async 0.2.0 interface is selected automatically for new artifacts, with 0.1.0
bindings retained for existing binaries. This does not add per-agent wrappers
or move workflow decisions into the host. The implementation record provides
the verification results and remaining gaps; E128 remains in place.


### Embed-owned timeout qualification · 2026-09-08

The normal inline Embed emitter now applies a total deadline across child work
and retries using the shared guest scope. Its own timeout returns
`EMBED_TIMEOUT` after cleanup and parent-frame restoration; inherited timeout
reasons bypass child recovery. Durable keys preserve the original budget across
parks and completed result checkpoints bypass later expiry. Retry sleeps and
Delay wakes stay within the enclosing budget.

Eight private-emitter tests cover pending I/O, nested/parallel children, parent
precedence, zero/overflow, replay, backoff, early wakes and malformed state. See
`compile/embed_deadline_tests.rs` and the implementation record. Public E128
rejection remains until the remaining tool/publication, race and grace contracts
are qualified; this is not a claim that all authored Embed timeouts are released.

### Inline Embed tool identity and timeout follow-up · 2026-09-08

Repeated inline Embed tools previously reused the child's step scope. The
candidate now derives a per-call namespace using the same guest helper as a
published workflow-agent tool, preserving the caller's definition path and
variables. The normal Embed path and tool path share deadline initialization,
exit checking and owned-error capture. No per-agent cancellation implementation
or host workflow dispatcher is added.

| Case | Candidate behavior | Test in `compile/embed_tool_deadline_tests.rs` |
| --- | --- | --- |
| Zero own budget | No child HTTP; non-retryable `EMBED_TIMEOUT` becomes tool feedback | `embed_tool_zero_budget_is_model_feedback_without_child_io` |
| Two calls to one tool | Distinct scopes and fresh budgets; hanging headers/body are closed | `embed_tool_pending_io_closes_and_next_call_has_fresh_budget` |
| Root Cancel or earlier parent expiry | Leave AI loop without feeding a child error to the model | `embed_tool_root_cancel_and_parent_timeout_bypass_model_feedback` |
| Earlier own expiry | Model may finish inside the still-live parent scope | `embed_tool_own_timeout_allows_model_to_finish_inside_parent_budget` |
| First call complete, second parked | Reuse completed result; early wake preserves the second call's original deadline | `embed_tool_resume_reuses_completed_call_and_original_pending_budget` |
| No timeout; ordinary failure then success | Keep normal error feedback and completion replay | `embed_tool_untimed_calls_preserve_errors_success_and_completed_replay` |
| Corrupt pending budget | Return non-retryable `EMBED_DEADLINE_STATE`; no new child I/O | `embed_tool_pending_call_rejects_corrupt_budget_without_new_child_io` |

The stdlib tests additionally check source preservation with a large payload,
legacy/v2 identities, replay equality, distinct counters/labels/AI steps, absent
variables and malformed input. Workflow-agent tool keys keep their existing
formula. Newly compiled inline Embed tools change checkpoint namespaces;
existing composed binaries are unaffected, and this is not a parked-workflow
checkpoint migration.

**Remaining replay gap:** an unfinished AI turn can call the model again on
resume. The test deliberately returns the same tool list and arguments. It does
not establish correctness if the model changes that list. Persist the model's
reply before dispatch, then test replay with a different scripted next reply.
The corrupt-budget test rejects 1-, 7- and 9-byte pending deadline records without new child I/O.
Nested AI loops inside these tool children also need explicit frame/arena
qualification. E128 remains until these and the remaining timeout contracts are
qualified. See the implementation record for checks actually run.

### Pending model decision replay and storage failures · 2026-09-08

The pending-response gap identified above is now fixed in the shared durable AI
loop. It saves the successful model response before any tools and reuses it after
suspension. An early resume cannot ask the model to choose different tool IDs,
arguments or order for the same partially executed turn. Completed-turn keys and
published workflow-agent tool identities retain their previous formulas.

| Case | Verified behavior | Execution test |
| --- | --- | --- |
| Response checkpoint read fails | Stop before model or child I/O | `ai_response_checkpoint_failures_prevent_tool_dispatch` |
| Response checkpoint write fails | Stop after the model response, before any child | Same test, write variant |
| Pause reported when response is saved | No tool starts; resume consumes that response | `ai_response_pause_after_persistence_replays_before_tool_dispatch` |
| Saved response is empty, malformed or lacks required fields | Non-retryable `AI_TURN_RESPONSE_STATE`; no model/child I/O | `ai_response_corrupt_checkpoint_fails_without_model_or_child_io` |
| Agent tool followed by a signal tool | Early resumes retain the wait identity; signal delivery preserves tool IDs, order and results | `ai_response_preserves_agent_and_signal_tool_decisions_across_resume` |
| Embed tool followed by a parked Embed tool | Restore the decision without another model request; retain the completed result and original pending budget | `embed_tool_resume_reuses_completed_call_and_original_pending_budget` |
| Ordinary Agent checkpoint storage fails | Failed lookup prevents I/O; failed save prevents successful continuation | `shared_checkpoint_errors_stop_ordinary_agent_execution` |

Checkpoint error propagation is shared with other durable step paths. A failed
result save cannot undo an already completed HTTP request; this change prevents
incorrect successful continuation, not duplicate external effects after a crash.
Provider calls interrupted before their response can be persisted may also run
again. Nested AI frames, full timeout qualification and external-effect
idempotency remain separate concerns.

New native tests verify distinct legacy/v2 response/completed-turn keys and
response shape validation, including large history, unknown tool indices and
argument values, malformed JSON, missing fields and integer overflow. The new
response checkpoint adds storage and validation cost; the plan now requires
explicit AI-loop measurements. No new performance result is asserted here.

### AUDIT-13 · Nested AI tools: planner identity and caller state

Two independently reproduced failures affected an outer AiAgent using an inline
Embed tool whose child contains another AI loop:

1. If the inner Agent or WaitForSignal tool reused the outer Embed's local step
   ID, the planner selected the child workflow by ID alone. A valid finite graph
   could recurse until the native compilation thread overflowed its stack and
   aborted the process. Tool selection now first checks the target's type in
   its own graph. Actual static child-closure cycles retain their existing
   rejection.
2. The inline child reused the outer AI loop's locals. The reproduction completed
   but returned only the second child result where two were expected. The
   existing Embed attempt boundary now saves/restores the caller's AI buffers,
   pending results, iteration/tool counters, conversation and heap watermark.
   Only the child's designated output locals escape that frame.

| Case | Result after the fix | Test in `compile/nested_ai_tests.rs` |
| --- | --- | --- |
| Two outer tools, with repeated local IDs and inner turns | Both original results and tool IDs reach the outer model | `nested_ai_preserves_two_outer_tool_calls_and_conversation` |
| More outer turns and 16–64 KiB histories | Caller and child histories remain separate; completed replay adds no calls | `nested_ai_large_histories_preserve_outer_turns_and_repeated_calls` |
| Child provider error | Original caller context receives `AI_TURN_COMPLETION_FAILED` | `nested_ai_error_returns_to_outer_model_with_original_context` |
| Nested signal wait and early resume | Both saved decisions and the wait identity survive | `nested_ai_wait_resume_preserves_both_decisions_and_original_signal` |
| Root Cancel, own Embed timeout or earlier parent timeout during inner model I/O | Root/parent reasons bypass outer model feedback; own timeout restores outer context for feedback | `nested_ai_cancellation_respects_owner_and_restores_parent_context` |
| Child loops reclaim large scratch values | Outer 64 KiB interned state and both tool results remain intact | `nested_ai_child_collection_keeps_outer_interned_state` |

Coverage includes pending headers/partial bodies and both durable/non-durable
execution where applicable. This adds no host state or per-agent wrapper. The
frame reuses existing locals and the guest operand stack; its code/stack overhead
still belongs in the planned size and latency comparison. Other inline callback
boundaries, deeper mixed recovery/parallel cases, full own AI budgets and cleanup
grace remain subject to the plan's qualification gates. E128 is unchanged.

### AUDIT-14 · Checkpoint failures outside the shared durable path

Three composed reproductions reported successful completion despite a storage
failure: an Agent attempt lookup treated the error as a cache miss, parallel
prelaunch discarded a transient lookup error and read again during assembly,
and a breakpoint write error allowed the step to run.

All emitted `get-checkpoint` and `checkpoint` calls now use two shared lowering
helpers. On error, a shared guest function preserves the original diagnostic,
resolves the active window's component calls with standard subtask cancellation,
and returns failure. It does not poll another signal while preserving this
error. Successful checkpoint signal handling remains at its existing safe
boundaries, including deferred retry handling. No host task bookkeeping or
per-agent cancellation wrapper is added.

| Case | Required behavior | Test in `compile/checkpoint_failure_tests.rs` |
| --- | --- | --- |
| Ordinary Agent or parallel Split attempt read fails | No fresh invoke; preserve the storage error | `attempt_checkpoint_read_failure_never_reinvokes` |
| Failed attempt cannot be saved | Preserve the write error; no retry or successful Finish | `attempt_checkpoint_write_failure_stops_retry_and_finish` |
| Parallel prelaunch has a one-shot read fault | Fail on that read rather than repeating it as a new lookup | `parallel_prelaunch_preserves_transient_checkpoint_failure` |
| Debug breakpoint cannot be saved | Do not execute the marked step | `breakpoint_checkpoint_failure_prevents_step_execution` |
| A read fails after another parallel call was queued | Resolve queued calls and report the original error | `checkpoint_failure_resolves_queued_parallel_calls` |
| One parallel branch finishes while its peer waits for HTTP headers/body, then result saving fails | Peer socket closes before the guest reports failure | `checkpoint_failure_resolves_live_parallel_io_before_reporting` |

The live-peer test gates the successful response on the other request already
being pending. Its `runtime.fail` callback requires the server's socket-close
notification while the Store still exists, so Store destruction alone cannot
pass it. This is a terminal storage-failure path: it does not establish selective
step timeout or sibling-preserving recovery. A failed write cannot undo the
completed external request. Retrying Split items retain the existing sequential
fallback; their tests do not claim concurrent retries. Malformed attempt payloads,
other non-checkpoint preparation failures and deeper mixed nesting need separate
qualification. No performance or bounded cleanup-grace claim is made here.


### AUDIT-15 · Agent budgets in parallel windows and error-scope cleanup

Timed Agents were excluded from parallel eligibility. The private timeout
emitter therefore serialized these graphs, so a passing sequential timeout test
could not establish that one invocation can expire while a sibling remains live.
The existing guest window now records each Agent's original budget in its slot
and uses one standard timer for the nearest pending Agent deadline. Returned
calls are retained in their slots for the ordinary scheduler to assemble. The
same lowering serves branch and Split invocations; agent exports remain entirely
on `agent_component!`, with no additional host task interface or agent wrapper.

The new composed fixtures also exposed an existing scheduler error-routing bug:
assembly and synchronous dispatch did not account for all enclosing Wasm blocks,
so a failure could be lost before reaching an enclosing handler. Correcting that
branch depth exposed another issue: leaving the scheduler could skip cancellation
of pending peers. A window-level error capture now invokes the shared window
cleanup before forwarding the original error to the enclosing scope.

| Case | Required behavior | Test in `compile/parallel_agent_deadline_tests.rs` |
| --- | --- | --- |
| Timed branch waits for HTTP headers or a partial body while an untimed peer remains pending | Cancel the timed call; the peer can return after observing that cancellation | `timed_branch_preserves_live_http_sibling_and_recovers` |
| A fast branch contains three successive calls while another Agent waits | Advance all three calls before the other Agent expires | `timed_branch_scheduler_keeps_fast_sibling_advancing` |
| Ordinary HTTP 503 leaves a scheduler for an outer `onError` handler | Preserve `HTTP_5XX`; close the pending sibling socket before recovery begins | `ordinary_branch_failure_cleans_live_peer_before_outer_handler` |
| Zero or maximum unsigned Agent budget, both branch execution paths and durability settings | Zero issues no HTTP for that Agent; maximum does not overflow; durable completed replay issues no new calls | `parallel_agent_zero_budget_and_maximum_budget` |
| Parallel Split with `dontStopOnFailed`, zero, finite and maximum item budgets | Aggregate each expired item as `AGENT_TIMEOUT`; maximum-budget items succeed | `parallel_split_aggregates_each_agent_deadline` |
| First Split item spends part of its budget resolving a connection while a later item starts fresh | Preparation consumes the first item’s budget; that item times out and the second succeeds after its socket closes | `parallel_split_preserves_peer_after_preparation_consumes_own_budget` |

The ordinary-error cleanup test enables step events and waits for the server's
socket-close notification at the recovery step's start, while the Store is still
alive. The sibling-survival fixture requires both actual requests to have started
before releasing the successful peer. The fast-chain fixture records four actual
requests before the target closes. These are executable composed-WASM checks,
not assertions about a handwritten model of the scheduler.

Qualification remains incomplete: cancellation during pending connection
preparation, parent/root versus completion
races, nested handled exits, concurrent retries and independently bounded cleanup
grace still need coverage. Public Agent/Embed timeout syntax continues to return
E128. The implementation adds 32 bytes to each parallel slot and three i32 values
to shared helper state; this is layout accounting, not a measured runtime-memory,
latency or binary-size result. Include these costs in the planned paired
benchmarks before making performance claims.


### AUDIT-16 · Earlier Agent deadlines during later connection preparation

A composed Split reproduction returned two `AGENT_TIMEOUT` errors when only the
first item needed to expire. The first item's HTTP request was pending, while
the second item's connection lookup waited for that socket to close. The
preparation wait watched its own deadline but did not service the earlier Agent's
budget; the second lookup therefore expired before the first was cancelled.

The shared guest Await helper now uses the active parallel window's waitable
set during preparation. It reuses the nearest-Agent timer and event handling:
an earlier expiry is recorded in that call's original slot, and a returned call
is buffered for normal assembly. Neither event ends the unrelated preparation
wait. Await resolves its own calls/timers on exit and leaves the shared set for
the window to close. Buffered calls remain detached until the scheduler drops
their resolved handles. No peer-handle transfers or additional preparation set
are needed. Closing a window clears its timed-window flag, preventing later
sequential waits from scanning old slots.

| Case | Required behavior | Test in `compile/parallel_agent_deadline_tests.rs` |
| --- | --- | --- |
| Later metadata response is held until an earlier HTTP call closes | Service the earlier Agent's deadline while the lookup remains live; later item succeeds | `pending_preparation_services_an_earlier_agents_deadline` |
| Earlier call returns while the later metadata lookup remains live | Buffer success and invoke each Agent exactly once | `pending_preparation_buffers_peer_success_without_reinvoking` |
| Root cancellation arrives with HTTP and metadata calls pending | Resolve both sockets before lifecycle acknowledgement; no item recovery | `pending_preparation_root_cancel_resolves_lookup_and_peer_before_ack` |
| Enclosing Split budget expires with HTTP and metadata calls pending | Resolve both sockets before reporting `SPLIT_TIMEOUT`; bypass item-error aggregation | `pending_preparation_parent_timeout_cleans_window_before_reporting` |

The metadata response is gated by the first call's completion/closure, rather
than released after an arbitrary sleep. Root acknowledgement and parent failure
reporting additionally wait for all fixture connections to close while the Store
is alive. Pending HTTP headers and partial bodies are covered. These cases reuse
the existing server fixture and public component interfaces; only the private
Agent-timeout emission bypasses E128.

No guest state fields, helper functions in the emitted core, WIT interfaces,
host tasks or agent wrappers are added. Await scans for the nearest deadline
when a timed window is active; helper code size and preparation latency still
need the planned paired measurements. Simultaneous completion/timeout/parent
races, deeper mixed handled exits, independent cleanup grace and public release
qualification remain open. E128 remains in place.

### AUDIT-17 · Async cancellation does not eliminate independent abort

**Status: runtime qualification; production behavior unchanged.** The current
engine rejects `canon subtask.cancel async`. Wasmtime 46.0.1 can enable it using
`wasm_component_model_more_async_builtins(true)`; that capability is enabled only
in these tests. No product flag, host task registry, or per-Agent implementation
is added.

The fixture composes a parent and a callback Agent in one Store. Its original
request remains pending until cancelled. Cleanup either waits for I/O, returns,
acknowledges cancellation, or enters an infinite CPU loop. The parent uses the
existing host-I/O timer import and standard waitable sets/subtask handles.

| Case | Observed behavior | Test in `cooperative_cancellation/async_cancel_grace.rs` |
| --- | --- | --- |
| Current production engine | Rejects the additional async-cancel ABI with the expected validation error | `async_cancel_requires_an_additional_engine_capability` |
| Cleanup awaits I/O, then acknowledges | Cancel returns `BLOCKED`; parent resumes before cleanup can complete; original handle resolves cancelled; same instance works twice | `async_cancel_allows_cleanup_ack_and_repeated_instance_use` |
| Cleanup returns normally | Original handle resolves returned; same instance remains usable | `async_cancel_allows_return_during_cleanup` |
| Cleanup I/O never completes | Parent observes grace expiry and traps; no false completion; Store teardown disposes the remaining I/O | `async_cancel_allows_guest_grace_during_pending_cleanup_io` |
| Cleanup callback loops forever | Parent never returns from async cancel; epoch yields do not deliver its timer; independent whole-run epoch interruption terminates execution | `async_cancel_still_needs_independent_abort_for_cpu_cleanup` |

`BLOCKED` is `0xffffffff`, not a terminal status or permission to drop a handle.
The caller must await resolution on the original subtask before dropping it.
The tests assert exact event order and actual trap types, and count pending host
futures before and after Store destruction. The CPU proof has a separately
running, joined epoch ticker and a bounded host watchdog.

**Consequence:** the extension is useful for cleanup that cooperates through I/O,
but cannot independently enforce grace against CPU cleanup. Keep the independent
host abort mechanism. Its integration with scoped deadlines must cover nested
and runtime-free workflows before enabling Agent/Embed timeout syntax. These
fixtures do not implement production grace, prove arbitrary native-call
interruption, or remove E128.

### AUDIT-18 · An independent cleanup alarm for the existing executor

**Status: native enforcement and shared deadline-wait integration implemented;
complete timed-scope ownership remains pending.** The
production host timer interface now includes `abort-after: async func(ms: u64)`.
Unlike `sleep`, expiry aborts the entire execution. It is a platform I/O/safety
capability whose ownership uses standard subtask handles, not a standardized
WASI operation or a host workflow scheduler. The existing `sleep` behavior and
default Component Model ABI are unchanged.

WASM must arm the alarm before potentially noncooperative guest work, with the
remaining scope deadline plus grace; it cannot wait for a blocked parent to
observe expiry. After normal completion or cleanup, it cancels and drops the
alarm using the ordinary ABI. Native expiry
latches an abort that cannot be undone. Disposal and expiry are synchronized so a
cancelled alarm cannot later set a stale abort flag. A native timer task services
each armed alarm independently of guest scheduling; one shared latch is checked
by the executor's epoch and blocked-I/O watchdogs. Guest CPU loops remain subject
to epoch interruption. This does not force-stop arbitrary blocking native code.

| Case | Required behavior | Evidence |
| --- | --- | --- |
| Unpolled alarm; expiry before Component Model can poll again | Independent native clock still sets the abort latch | `cleanup_alarm::tests::expiry_is_independent_of_polling_the_component_future` |
| Cancel/dispose before expiry, including before first poll | No late abort; native timer releases its references | `dropping_an_unpolled_alarm_disarms_and_releases_its_native_timer` |
| Multiple alarms and a selected expiry | Disposing one preserves the others; disposal cannot clear an abort | `cancelling_one_alarm_preserves_another_and_expiry_is_latched` |
| Zero, maximum and repeated disposal | Zero is immediate; maximum can be cancelled; 1,000 disposed alarms release their state after scheduler reaping | Remaining `cleanup_alarm::tests` |
| Synchronous cancel enters CPU-bound or pending-I/O cleanup | Current production executor returns `CleanupAborted` after Store teardown | `workflow::cleanup_alarm_tests::production_alarm_aborts_*` |
| Guest body prevents the parent from regaining control | Alarm armed before the call still bounds execution | `production_alarm_bounds_cpu_work_before_parent_can_select_timeout` |
| Command and dispatcher entry points | Apply the same alarm through their existing execution guards | `production_alarm_also_bounds_command_execution`, `production_alarm_also_bounds_dispatcher_execution` |
| Successful cleanup | Standard cancellation disarms the alarm; same Store runs past its former deadline | `standard_cancellation_disarms_alarm_and_preserves_the_same_store` |
| Expiry wins immediately before normal return | Discard the late successful output | `latched_cleanup_abort_rejects_a_late_successful_return` |
| Runtime complete/fail after alarm expiry, both supported ABI versions | Reject the host call before publication; unexpired calls still publish | `expired_cleanup_alarm_rejects_new_runtime_publication_in_both_versions` |
| Persist aborted execution, with/without a pending Cancel | Failed or cancelled with termination reason `aborted`; no command acknowledgement | `runner::embedded::tests::cleanup_abort_records_unclean_failure_or_cancel_without_acknowledgement` |
| Accepted terminal state | Preserve output, error, finish time, termination reason and exit code | `cleanup_abort_preserves_accepted_terminal_outcomes` |

The composed executor fixture imports no workflow runtime and uses synchronous
`subtask.cancel`, proving that this enforcement does not require optional async
cancellation or persistence-backed workflow orchestration. An added negative
control, `ordinary_timer_trap_does_not_interrupt_cpu_cleanup`, rejects the tempting
shortcut of throwing from a normal Component Model timer future.

The shared runtime host accessor rejects new runtime calls after a latched abort,
including terminal writes and signal acknowledgements. The new regression fails
without that guard: native return rejection alone still allows a late durable
publication. A call already started before expiry retains its existing outcome
semantics; no rollback or preemption of arbitrary native code is implied.

The shared deadline-wait emitter now arms this alarm before sequential Agent
entry, connection preparation and enclosing-scope window waits. It uses the
remaining monotonic deadline plus a saturating five-second grace, matching the
current default root Stop grace. It retains the alarm through timeout cleanup
and disposes it after success. Parent cancellation and observed root Cancel
relinquish the local alarm before cleanup; no fresh local grace replaces the
initiating owner's deadline.

The existing seven helpers carry 20 i32 state values (one added alarm handle).
The compiler's timer WIT adds `abort-after`, and the cache tag is
`cooperative-waits=shared-v15`. No Agent export or binding changes are needed.
New compiler artifacts require the updated host timer interface; old artifacts
continue importing their existing subset.

Tests in `deadline_cleanup_tests.rs` exercise real emitted/composed workflows
with an infinite Agent body, an infinite cancellation callback, and a successful
timed call followed by six seconds of untimed HTTP work in the same Store.
Deterministic shared-helper tests check cleanup/disposal order for own timeout,
window timeout, success and root/parent propagation.

At this stage parallel launches still needed alarms retained for each pending call, including
while another call is prepared. Enclosing inline scopes need uninterrupted
coverage across dispatch and assembly boundaries, including preparation-timeout
propagation into cleanup of the entire window. These remain prerequisites
for removing E128; shared-wait coverage alone does not establish full scope
grace. Per-Store latch allocation, armed-alarm cost and cancellation races remain
part of the full performance/soak gates.

### AUDIT-19 · Parallel calls retain their cleanup alarms

**Status: per-call ownership implemented; continuous enclosing-scope ownership
and final qualification remain open.** Each actual launch in the scheduler,
wavefront and parallel Split arms an alarm through the shared deadline selector.
Its duration uses the original earliest own/inherited remaining budget plus the
existing five-second grace. The call keeps that alarm while other work is being
prepared or awaited. It is disposed on eager/observed return or after timeout
cleanup; root/parent propagation relinquishes window alarms before cleanup.

The handle uses existing slot padding at offset 204, preserving the 208-byte
stride. The helper count and 20 i32 state values are unchanged. The cache tag
advances to `cooperative-waits=shared-v16`; this stage changes neither Agent
components nor host interfaces.

| Case | Required behavior | Evidence |
| --- | --- | --- |
| Own Agent deadline; CPU loop before launch returns | Prearmed alarm aborts before the separate run timeout in all three schedulers | `parallel_launch_alarms_precede_cpu_bound_entry_in_all_schedulers` |
| Own Agent deadline; CPU loop in cancellation callback | Call alarm survives synchronous cancellation and bounds cleanup | `parallel_call_alarms_remain_live_during_cpu_bound_cleanup` |
| Inherited While/Split deadline; CPU loop on entry | Same enforcement using the enclosing budget, before window-wait arming | `inherited_parallel_deadlines_prearm_before_cpu_bound_entry` |
| Inherited deadline; CPU loop during cleanup | Whole-run abort without a false cleanup acknowledgement | `inherited_parallel_deadlines_bound_cpu_bound_cleanup` |
| Timed call returns; untimed peer runs another six seconds | Returned call's disposed alarm cannot abort continued work in the same Store | `returned_parallel_call_disarms_alarm_while_untimed_peer_stays_live` |
| Timed call returns during another connection lookup that stays blocked for six seconds | Dispose while preparation is pending, before returning to ordinary window scheduling; require normal success output | `returned_parallel_call_disarms_alarm_during_peer_preparation` |
| Unrelated wait, ordinary return and timeout cleanup | Peer alarms remain live; each alarm resolves with its own call | `emitted_call_alarms_survive_other_waits_and_resolve_with_their_calls` |
| Root or parent cancellation | Disarm window alarms before cleaning up calls, preserving initiating grace | `emitted_root_and_parent_cancel_disarm_all_call_alarms_before_any_cleanup` |

A temporary negative control removing disposal from `remember_returned` fails
the pending-preparation test with `CleanupAborted` at 5.505s. With disposal restored,
both normal-success cases pass. A failed workflow stops its fixture rather than
waiting for requests that it can no longer dispatch, exposing the actual exit.

At this stage continuous scope coverage through inline assembly/checkpoint work and error
propagation remains a prerequisite for removing E128. Actual alarm allocations,
size/latency comparisons, cancellation races and Linux soak remain part of the
full plan gates; unchanged guest slot size does not establish unchanged runtime
memory or performance.

### AUDIT-20 · Enclosing scope alarms survive work between calls

**Status: effective enclosing-scope ownership implemented; full release
qualification remains pending.** One alarm per emitted invocation follows the
earliest live enclosing deadline throughout scope work. An earlier nested deadline replaces it, and
scope restoration reinstates the parent's original deadline plus remaining
grace. It is not a stack of native tasks or a host graph registry. Existing
shared scope frames provide the owner, start and budget.

Grace restoration uses `max(0, saturating_add(budget, grace) - elapsed)`.
Subtracting elapsed time after adding grace prevents overdue parents from gaining
a fresh cleanup period. Completion, failure and suspension use shared ABI exits
to dispose the alarm. Root/parent cancellation relinquishes it before resolving
nested calls, preserving the initiating grace.

Timed Agent-free workflows now derive the existing timer import from their live
deadline requirements. A disabled While/Split budget does not require a timer or
clock solely for that budget. The helper count remains seven; helper state has
21 i32 values, with one i32 alarm handle and two i64 scratch locals added to the
entry frame. Parallel slots remain 208 bytes. The cache tag is
`cooperative-waits=shared-v17`.

| Case | Required behavior | Evidence |
| --- | --- | --- |
| While/Split completion checkpoint blocks with no Agent calls | Original scope deadline plus grace aborts the execution | `scope_alarm_bounds_checkpoint_without_pending_agent` |
| Child completes, parent checkpoint blocks; all four While/Split nesting combinations | Dispose child's alarm and restore parent's budget | `parent_scope_alarm_is_restored_after_nested_scope_completion` |
| Success, ordinary error or timeout followed by six seconds of untimed HTTP | Normal continuation succeeds after scope disposal; timeout closes its HTTP request before recovery | `scope_alarms_dispose_before_untimed_success_and_error_continuations` |
| Parent restored after its ordinary deadline, or beyond grace | Original clock consumes grace; no reset and saturating arithmetic | `restored_scope_grace_uses_original_clock_and_saturates` |
| Root/parent cancellation with scope, wait and call alarms | Relinquish local alarms before nested cleanup | `emitted_root_and_parent_cancel_relinquish_scope_alarm_before_cleanup` |
| Zero-disabled While/Split budget | Compiled world gains no clock/alarm import solely for that budget | `disabled_loop_budgets_do_not_add_alarm_imports` |

The blocked-checkpoint regression initially returned the ordinary ten-second run
timeout. With scope ownership it returns `CleanupAborted` during the scope's grace
limit, without a false acknowledgement. These proofs add scope coverage; they do
not establish arbitrary native-call preemption or close performance, Linux/soak,
authenticated server E2E and the remaining G1–G10 release gates. E128 remains.

### AUDIT-21 · Parallel deadline selection under simultaneous readiness

**Status: deterministic emitted-helper race contract covered; no production
behavior or public support gate changed.** The shared canonical-event fixture now
also executes the real `WindowWait` helper with per-Agent deadlines enabled,
two live call slots, per-call alarms and an effective enclosing-scope alarm.
The existing tests primarily exercised an operation or enclosing-window timer;
these cases exercise the distinct per-Agent timer/ready-slot path.

| Case | Required behavior | Evidence |
| --- | --- | --- |
| Target completion, its timer and peer completion all ready | Every notification order preserves both successful results; calls stay owned by their slots for scheduler assembly and alarms are disposed | `parallel_ready_completion_beats_own_deadline_in_every_event_order` |
| Target timer and successful peer ready; target finishes while cancellation is resolving | Selected `AGENT_TIMEOUT` overwrites the late success retptr; peer result survives, and both call handles remain for single scheduler disposal | `parallel_selected_timeout_survives_late_success_and_preserves_ready_peer` |
| Enclosing deadline, Agent timer and peer completion all ready | Every order selects the enclosing scope outcome, resolves the window and balances its deferral; scope alarm remains until enclosing unwind | `parallel_enclosing_timeout_owns_pending_window_even_with_ready_peer` |
| Root Cancel observed before own-timeout selection, or parent cancellation delivered at a cancellable wait | Root/parent outcome owns cleanup; scope alarm is relinquished before resolving the calls, with no Agent timeout result substituted | `parallel_root_and_parent_cancel_take_ownership_before_local_timeout` |

The four test groups cover 21 controlled notification/cancellation combinations,
including normal return, start-cancelled and cancelled results from standard
synchronous cancellation. They check result tags/error code, handle ownership,
waitable-set membership, alarm disposal and scope deferral. Fixture instantiation
is shared with the prior helper tests. Detaching/dropping a fixture waitable
removes its queued notifications, so a disposed timer cannot produce an impossible
stale event. A late normal return writes a success to the actual target slot's
result area before cancellation returns.

A temporary negative control omitting the production timeout-result write fails
the late-success regression (`0` success tag instead of `1` error tag). Restoring
the production emitter makes it pass. This establishes sensitivity to timeout
selection being lost; the fixture does not stand in for engine-level cancellation
or HTTP cleanup tests. Root cancellation here is observed at the existing
pre-timeout poll; this does not claim that a signal preempts an already accepted
completion at every instruction.

Retry inventory: `parallel_agent_body` and `concurrent_branch_pools` exclude
retrying Agent bodies, and Split-level retries also use sequential fallback.
`concurrent_backoff` is always false under production eligibility. Qualifying
cancellation must preserve those existing retry/replay semantics; enabling a
new concurrent durable retry scheduler is not a prerequisite for this change.
The unreachable old lowering still needs cleanup in the obsolete-path inventory.

Verification for this test-only stage is recorded in the implementation record.
E128, composed runtime race qualification, final measurements and the other
remaining release gates stay open.

### AUDIT-22 · Retire unreachable concurrent Split retry lowering

**Status: dormant emitter path removed; supported retry behavior preserved.**
Production eligibility has always set `concurrent_backoff` to false in this
implementation. Retrying Agent bodies and retrying Splits select the existing
sequential lowering, which owns durable retry parking and replay. The old
classify/backoff/reinvoke rounds could not be selected, but still carried a
second retry implementation with its own slot states and checkpoint helpers.

The emitter now removes those unreachable rounds, the constant-false selector,
its assembly indirection, unused retry envelope/copy helpers and the unused
rate-limit slot-offset constant. Active launch/assembly, attempt-1 checkpoint
lookup, slot layout, scope alarms and sequential retries remain unchanged. No
host code, WIT, migration, runtime capability or old-artifact loader is removed.
Existing compiled artifacts retain their own emitted logic; parked runs are not
recompiled by this cleanup.

A before/after capture compared **32 uncomposed compiler artifacts** from the
same Split fixture: durable/non-durable execution, Agent retries 0/2, Split
retries 0/2, parallelism 1/4 and enclosing timeout 0/4000 ms. Every artifact is
byte-identical. [Comparison metadata and hashes](research/cooperative-retry-retirement-comparison.json)
record the baseline revision, candidate source hashes, input fixture and matrix.
This is compiler-output equivalence for that corpus, not a performance result
or blanket proof for every possible workflow. The cache tag and slot stride
therefore remain unchanged.

Existing regression coverage used for this cleanup includes:

| Contract | Existing evidence |
| --- | --- |
| Requested parallel Split with retrying item parks, then resumes exactly one retry | `direct_wasm_execute_invoke_parallel_split_item_retry_parks_sequentially` |
| Retrying shapes remain accepted and compile through fallback | `direct_wasm_compiles_parallel_split_durable_rate_limited_retries`, `direct_wasm_compiles_non_durable_parallel_split_retries`, `direct_wasm_compiles_parallel_split_durable_retry_replay_shape` |
| Eligible no-retry Split and branches retain real HTTP overlap | `direct_wasm_execute_parallel_split_http_overlap`, `direct_wasm_execute_parallel_branches_http_overlap` |
| Pause/replay preserves completed effects | `direct_wasm_execute_parallel_split_pause_mid_window_resumes`, `direct_wasm_execute_parallel_branches_durable_resume_no_double_fire` |

Final commands and results are recorded in the implementation record. The broader
obsolete isolation/task inventory and artifact compatibility gates remain open;
removing this unreachable retry lowering does not complete them or retire E128.

### AUDIT-23 · Authenticated server cancellation and unresolved peer-Stop failure

**Status: single-server cases pass; the two-server reproducer fails. No production
fix is included in this progress snapshot.** The new
[`e2e/test_cooperative_cancellation.py`](../e2e/test_cooperative_cancellation.py)
uses the real server, public workflow create/compile/execute/Stop APIs, normally
composed WASM and the HTTP proxy. It generates an isolated OIDC/JWKS fixture in
memory and starts fresh PostgreSQL and Valkey containers. It does not require
existing credentials. Cleanup stops only its own processes and containers;
fixture databases and logs are retained.

| Case | Required behavior | Observed result |
| --- | --- | --- |
| Stop while HTTP response headers are withheld | Close the request, persist `cancelled`, acknowledge the signal and remove the execution registry entry | Pass on the owning server |
| Stop while a partial response body stalls | Same cooperative cleanup contract | Pass on the owning server |
| Missing/invalid authentication, wrong tenant, or viewer role | Return 401/403 without persisting cancellation or closing the request | Pass in both single-server cases; checks also pass before the failing peer Stop |
| Retrying HTTP with success/error continuations | Cancellation must produce neither a retry nor either continuation | Pass in the completed single-server cases |
| Duplicate Stop after cancellation | Return success without another launch | Pass in the completed single-server cases |
| Start peer B after server A reaches pending HTTP, then Stop through B | Reach the live execution and close its request within the four-second fixture bound | **Fail:** Stop returns HTTP 200, but A's request stays open beyond the bound |
| Peer Stop during a partial response body | Same cross-server contract | Not reached: the script stops at the preceding failed assertion |

The first completed single-server run reported 1.018s (headers) and 1.027s (body).
The expanded run repeated those passes at 1.072s and 1.047s before failing the
peer/header case. These are individual fixture durations including status checks
and duplicate Stop, not controlled cancellation latency benchmarks. A passing
case requires pending signals to be acknowledged, no remaining registry row, and
a termination reason other than `aborted`; emergency termination cannot count as
proof of cooperative cleanup.

The cross-server setup starts B only after A's HTTP request is pending. B shares
the isolated databases and data directory. This also exercises startup recovery,
so the failure does **not yet isolate** signal delivery, runner ownership or
startup reconciliation as the cause. A successful Stop response alone does not
establish that the active request was cancelled. The next implementation work is
to inspect the persisted launch/registry state and recovery path, fix the existing
whole-execution lifecycle boundary, and rerun all four cases. This finding does
not justify introducing host-side workflow graph orchestration or per-step task
management.

Verification recorded for this stage:

- `cargo build -p runtara-server --bin runtara-server` passed using the pinned
  toolchain, `SQLX_OFFLINE=true`, the isolated native target and previously built
  Agent components (5m44s).
- The initial two-case authenticated E2E run passed; the expanded four-case run
  failed as described above. The reproducer deliberately retains its failing
  assertion and is not silently skipped or treated as a passing test.
- Python syntax and `git diff --check` are checked for this snapshot. No new
  production Rust, WIT or migration changes are included; the full Rust suite,
  component builds, final benchmarks and Linux soak are not rerun for it.

To reproduce with a built server and components:

```sh
python3 -u e2e/test_cooperative_cancellation.py \
  --server /absolute/path/to/runtara-server \
  --components /absolute/path/to/wasm32-wasip2/release
```

Prerequisites are Docker with `pgvector/pgvector:pg18` and
`valkey/valkey:8-alpine`, Python with `cryptography`, and the matching server and
Agent builds. The script prints its retained fixture directory. This snapshot is
on `feat/cooperative-cancellation-progress`, separate from `main`; upstream
integration, the PR and the other completion criteria above remain outstanding.

#### Follow-up: the failed Stop targeted a replacement launch

Read-only inspection of the retained isolated database confirms that the failing
instance had **two physical launches**: the original was suspended, a replacement
was cancelled, and `recovery_attempts` increased to one. Unlike the two passing
instances, it had no persisted Cancel signal row. Startup's
`recover_orphaned_containers` scans the shared registry and assumes every entry
belongs to a dead predecessor. Starting B therefore recovered A's still-live
run. Stop then succeeded through pre-start cancellation of the replacement.

The E2E now records the running generation before starting B and checks that
startup preserves status, registry handle/launch, launch count, recovery counter
and pending signals **before** sending Stop. This distinguishes startup damage
from remote signal/grace delivery. The production Stop handler also still arms
grace only on a locally owned runner handle; fixing startup alone does not
qualify cross-server grace. Drain and heartbeat scans also need ownership review.

With the AUDIT-24 host fix and a rebuilt server, owning-server cases again pass
(headers 0.962s, body 0.971s). The strengthened regression fails **before Stop**:
peer startup changes the live instance from `running` to `suspended`. The peer/body
case remains unreached. This is still a failing E2E, not expected-success coverage.

### AUDIT-24 · Stale recovery must preserve accepted lifecycle outcomes

**Status: the recovery-write race is fixed; multi-owner lifecycle handling is
still incomplete.** Recovery previously updated an instance by ID without
checking its current state. A scan could observe `running`, then overwrite an
accepted cancellation/completion or a newer suspension with `suspended`,
`environment_restart` and an immediate wake. The new PostgreSQL regression
failed against that implementation: a `cancelled` row became `suspended`, its
termination reason changed and its wake/counters were replaced.

`mark_for_recovery` now updates only a row still in `running`, atomically with
the recovery fields, and returns whether it applied. `recover_or_fail` reports
`Unchanged` when either its recovery or terminal-failure write loses to another
transition. Database write failures propagate as errors instead of being reported
as terminal failures. Startup retains tracking after an error or unapplied
decision. Its successful cleanup uses the existing exact physical-handle guard
so it cannot delete a replacement registry row selected after the scan.

| Contract | Test evidence |
| --- | --- |
| Cancelled/completed/failed/suspended/pending state wins after the scan | `recovery_write_preserves_states_that_changed_after_scan` preserves status, termination, wake, result/error, finish timestamp and recovery counters; both recovery policies report `Unchanged` |
| Running instance is still recoverable or fails when recovery is disabled | `recovery_reports_applied_outcome_once` checks applied state and repeated decision without a second mutation |
| Database failure is not evidence of terminal failure | `recovery_write_failure_is_not_reported_as_terminal_failure` checks both policies return errors |
| Cleanup must retain a replacement registry entry | `cleanup_handle_preserves_every_replacement_identity` checks replacement of handle, launch, or both, followed by successful exact cleanup and idempotent repetition |

This is a host lifecycle persistence correction. It changes no guest graph
control, Agent macro, WIT, cancellation ABI, component binary, schema or feature
flag. A status guard cannot distinguish two different physical executions that
both appear `running`; owner/generation fencing and the peer-startup failure
remain open. Verification commands/results are in the implementation record.

### AUDIT-25 · Runner control must identify a physical execution

**Status: physical-handle aliasing fixed; server ownership remains unresolved.**
The embedded runner used the durable `launch_id` as the key for its task and
occupancy maps, and derived `handle_id` from that same ID. Queue recovery may
reuse a launch before guest start. Consequently, after one physical execution
retired and a replacement was installed, an old handle could see the replacement
as running, arm its emergency timer, stop it or wait for its exit. The persisted
registry's exact-handle guard could not distinguish two identical derived IDs.

Each accepted runner handoff now generates an opaque unique physical handle.
The existing task map, run-permit accounting and completion guard use that key;
`is_running`, `wait_for_exit`, `stop` and `schedule_abort` resolve it. The durable
launch ID still identifies queue state and is not parsed from the handle.
There is no new task registry, per-step task, guest import, workflow artifact,
schema change or product flag. The mock runner follows the same identity rule,
including physical result lookup, so lifecycle tests do not retain the old alias.

| Contract | Evidence |
| --- | --- |
| A retired handle cannot address a later execution of the same durable launch | `retired_handle_cannot_control_a_reused_durable_launch` runs real spinning WASM, retires it, reuses the launch ID and exercises old liveness/grace/Stop/wait calls; the new run must survive |
| Old handoff cleanup cannot remove replacement occupancy | `overlapping_handoffs_keep_separate_physical_handles_and_occupancy` installs two closed gates under one launch ID, retires the first and checks that the second retains its handle, permit and age until its own gate is cancelled |
| Mock behavior must preserve the same boundary | `reused_launch_preserves_physical_results_and_control` checks old/current results, old Stop/grace and completion of the current execution |

Before the fix, the real regression observed `(old looks live, old grace accepted,
replacement still live) = (true, true, false)`. After the fix it requires
`(false, false, true)` and prompt retirement of the old wait. Tests also keep the
durable ID equal while requiring distinct physical IDs; allocating a different
durable launch would not prove the recovered-attempt case.

This corrects a prerequisite for safe ownership/grace routing. It does not tell
startup which server is alive, renew ownership, or deliver a remote grace
deadline. AUDIT-23, the wider lifecycle/compatibility work and G1–G10 remain open.
See the implementation record for verification and skipped gates.

### AUDIT-26 · Execution ownership lease implementation snapshot

**Status: implemented with focused regression coverage; multi-server behavior is
not yet qualified.** This checkpoint saves the current work on
`feat/cooperative-cancellation-progress`, separate from `main`. It includes the
ownership implementation and tests, not just this document. The latest server
E2E evidence still comes from the preceding physical-handle stage (`611507c7`):
owner/header and owner/body cases passed at 0.968s and 0.984s, then peer startup
changed the live instance from `running` to `suspended` before Stop. Those timings
are individual fixture observations, not controlled benchmarks.

The implementation reuses `instance_launches.lease_owner` and `lease_expires_at`
through the running state. The existing physical-run monitor renews that claim
for 30 seconds every 10 seconds, checking owner, attempt, physical handle and
unexpired ownership. It uses a conservative local deadline, one second before
its renewal interval expires, and retries transient database errors within that
bound. A blocked renewal future cannot indefinitely retain execution: losing
ownership invokes the existing whole-execution abort and closes an unopened
start gate. Renewing ownership does not extend the original start-gate deadline.

Startup recovery locks the observed durable launch and exact physical registry
row before checking expiry and applying a Core recovery decision. Live claims
and replacement handles are retained. The existing heartbeat pass also scans
expired running owners independently of workflow event age; a new forward
migration indexes that scan. Drain selects handles owned by the local runner.
These are execution lifecycle responsibilities. No new per-step registry,
workflow scheduler, guest ABI, component binary or product feature flag is added;
the existing monitor polls renewal alongside execution completion.

| Behavior | Regression test |
| --- | --- |
| A live claim survives a peer recovery attempt; only its exact owner, attempt and physical handle may renew | `live_running_owner_is_retained_and_cannot_be_recovered_by_a_peer` |
| An expired claim cannot renew; recovery suspends it once and removes its exact registration | `expired_running_owner_cannot_renew_and_is_recovered_once` |
| A stale recovery snapshot cannot mutate a replacement physical handle | `expired_owner_snapshot_cannot_recover_a_replacement_handle` |
| Exhausting the monitor's database pool still aborts real spinning WASM at the local ownership bound; normal cleanup resumes when the pool is released | `blocked_lease_database_cannot_keep_a_physical_guest_running` |

Remaining risks and required follow-up:

- Rebuild the server and rerun all four authenticated header/body, owner/peer
  scenarios. The last failing E2E is not converted into a pass by these focused
  ownership tests. Stop still arms emergency grace through a local runner handle;
  peer request delivery and grace routing remain unfinished.
- Qualify sustained renewal, expiry while waiting for a row lock, peer drain,
  periodic recovery after recent heartbeats, and concurrent lifecycle transitions.
- A database outage can now end a whole execution when ownership cannot renew.
  Recovery latency includes the lease duration and the existing heartbeat scan
  cadence. Pauses of the native process and host/database clock changes need
  explicit operational qualification.
- Recovery holds a database connection and row locks while Core persistence uses
  another connection. Small-pool and concurrent-recovery behavior still needs
  capacity testing. Measure renewal writes, index maintenance and database load
  in the final paired benchmark report.
- Legacy registrations without a lease retain the historical recovery fallback;
  this does not establish mixed-version multi-server safety.
- Public Agent/Embed timeouts still return E128. The remaining G1–G10 work,
  artifact compatibility/obsolete-path cleanup, final size and latency comparison,
  Linux soak, recent-upstream integration and PR remain open. The conflicting
  committed migration numbers 025/026 have not been rewritten or resolved.

Verification on this source snapshot:

- `cargo test -p runtara-environment --features scoped-workflow-integration-tests
  --lib --test embedded_runner_test --test launch_queue_test --test handlers_test
  --test heartbeat_monitor_test --test cooperative_stop_test
  --test container_registry_test -- --test-threads=1` passed **349 tests**:
  244 library, 11 embedded runner, 12 launch queue, 45 handlers, 21 heartbeat,
  four composed Stop and 12 container registry tests. The new blocked-pool
  spinning-WASM regression passed. Tests used the isolated PostgreSQL fixture,
  pinned toolchain and existing matching Agent component build.
- Feature-gated environment all-target Clippy with `-D warnings` and
  `git diff --check` passed. The commit uses the normal pre-commit hook, which
  requires workspace formatting and workspace all-target Clippy; no bypass.
- This is a progress checkpoint. The server was not rebuilt and authenticated
  server E2E was not rerun with the lease change. The full compiler/component
  matrices, final paired benchmarks and Linux soak were also not rerun.


### AUDIT-27 · Deliver emergency grace to the physical owner

**Status: the four-case authenticated owner/peer HTTP regression now passes.**
Rebuilding `b932f1c6` first established that peer startup preserves the live run;
Stop then returned HTTP 500 because only the owner could arm native emergency
grace. The present change delivers that whole-execution deadline across peers.
Cooperative cancellation still uses the existing persisted Cancel signal and
standard guest cancellation. There is no new guest task manager or graph logic.

The existing physical `container_registry` row now holds the requested emergency
deadline and the deadline acknowledged by its owner. The forward migration adds
two nullable timestamps and an index for pending delivery. The existing launch
dispatcher processes a bounded batch before queue work, including while draining;
no new worker or per-step registry is introduced. It selects its own live claims,
arms the exact native physical handle, then records acknowledgement. Registry
replacement clears these fields; an upsert of the same handle preserves them.

A peer Stop waits at most five seconds for timer acknowledgement or an accepted
terminal outcome. Persisting a request alone does not return success. Missing
owner confirmation returns an error while leaving the already delivered Cancel
signal and deadline request intact. The five-second wait bounds the HTTP-side
confirmation; it does not grant more cleanup time. Zero grace remains due
immediately upon delivery, without allowing a new zero-to-five-second grace.
Duplicate requests keep the earliest deadline, and acknowledgement of a later
deadline cannot satisfy an intervening shorter request.

Deadline conversion samples the database clock, subtracts time already spent
from the caller's monotonic budget, and persists a fixed timestamp. The owner
converts the remaining database interval from before its read began; network,
query and row-wait latency therefore do not restart a full grace interval.
Transport/poll delays can still make an already-due abort arrive late. Cross-host
wall clocks are not compared, but database clock jumps and process pauses still
require operational qualification; this is not a real-time delivery guarantee.

| Contract | Evidence |
| --- | --- |
| A peer must wait for the exact owner to arm grace | `peer_stop_waits_for_physical_owner_to_arm_emergency_grace`; wrong owner and a runner without the handle cannot acknowledge |
| Unconfirmed delivery must not be reported as success | `peer_stop_does_not_claim_delivery_when_owner_never_confirms`; zero-grace request remains unsuccessful after the bounded wait |
| Delayed and repeated requests cannot restart grace or target a replacement | `remote_grace_never_extends_and_cannot_follow_a_replacement_handle`; checks shorter/later requests, same-handle upsert, overdue delivery and physical replacement |
| Remote emergency grace stops real non-cooperative WASM | `peer_stop_grace_aborts_spinning_invocation` and `peer_stop_zero_grace_aborts_infinite_initializer`; require actual exit, zero occupancy, `aborted` outcome, no guest cleanup acknowledgement and a healthy subsequent invocation |
| Authenticated peer Stop cooperatively closes pending HTTP | `e2e/test_cooperative_cancellation.py`; owner and peer, headers and partial body, unauthorized callers, duplicate Stop, no retries/continuations, signal acknowledgement and registry cleanup |

The first complete four-case run reported 1.017s / 0.849s on the owner and
0.545s / 0.587s through a peer (headers / body). These are individual fixture
durations, not controlled latency measurements. An extended run additionally
holds the peer/header request for 32 seconds, checks renewal beyond the original
running lease, and shuts down an unrelated peer before sending Stop. The HTTP
fixture explicitly uses a 90-second I/O timeout so its own default 30-second
request timeout cannot mask ownership behavior. That extended run passed renewal,
unrelated peer shutdown and all four cancellation cases: 0.980s / 1.007s on the
owner and 1.119s / 0.640s through a peer. These remain fixture observations.

Focused validation covers 354 distinct environment tests across the listed
suites: 244 library, 45 handler, 15 launch queue, 12 registry, four composed Stop,
13 embedded runner and 21 heartbeat tests. The initial broad run exposed a
non-unique image name in the new native fixture; after making it unique, all 13
embedded and 21 heartbeat tests passed. See the implementation record for
commands, final extended-E2E result, lint and limitations.

This closes the reproduced peer-startup/Stop failures, not G1–G10. Public
Agent/Embed timeouts still return E128. Full owner-loss/recovery and partition
coverage, mixed-version safety, the remaining composed race/CPU/durable nesting
matrix, obsolete-path compatibility work, controlled size/time/DB-cost reports,
Linux soak, upstream migration integration and the PR remain outstanding.


### AUDIT-28 · Agent tools need real guest deadlines

**Status: Agent-as-AI-tool enforcement implemented; E128 remains.** Public-timeout
review found that `DirectAiToolPlan::Agent` carried `timeout_ms`, but the tool arm
only called `ai-tool-args-with-timeout`. Its invocation site was excluded from
Agent deadline selection. This could modify an HTTP capability's own timeout
argument; it did not establish workflow-owned cancellation or a predictable
per-call budget. Removing E128 without correcting this path would expose that
mismatch to authored workflows.

The tool plan now carries its definition step and effective durability. A small
guest emitter helper reuses the existing Agent budget, scoped source and
checkpoint operations. It identifies a tool call using the existing AI step,
tool label and replay-stable call counter. The shared Agent invoke/connection
preparation path selects the earliest own or enclosing deadline and resolves
standard cancellation before returning timeout feedback. The old argument-merge
call and its unused compiler import-index plumbing are removed; the stdlib export
remains available to previously compiled artifacts.

A durable result checkpoint bypasses both invocation and its old budget on
replay. A pending budget survives pause and includes elapsed parked time; a new
model-selected call starts its own budget. Own timeout becomes a non-retryable
`AGENT_TIMEOUT` tool result. The model may explicitly select another call, while
root cancellation or enclosing timeout escapes the tool loop without feedback or
new model work. Capability arguments retain their own meaning, including the
HTTP I/O timeout. The compiler cache tag advances from `shared-v17` to
`shared-v18`; old stored workflow artifacts are not rewritten.

| Contract | Test |
| --- | --- |
| Zero budget sends no HTTP and yields typed, non-retryable feedback | `agent_tool_zero_budget_is_typed_feedback_without_invocation` |
| Pending headers/body close before subsequent model work; a later selected call has a fresh budget; the HTTP timeout stays 90,000 ms | `agent_tool_closes_headers_and_body_and_gives_next_call_a_fresh_budget` |
| Root Cancel and enclosing While timeout bypass tool feedback | `agent_tool_root_cancel_and_parent_timeout_bypass_model_feedback` |
| Completed calls replay after pause without another effect or model decision | `agent_tool_completed_calls_replay_after_pause_without_reinvoking` |
| A pending budget expires while paused; only a new second call may invoke | `agent_tool_pending_budget_survives_pause_without_granting_extra_time` |
| Malformed durable budgets fail before new tool I/O or model feedback | `agent_tool_corrupt_pending_budget_fails_before_dispatch_or_model_feedback` |
| Maximum unsigned budget preserves success and provider errors | `agent_tool_maximum_budget_and_provider_errors_preserve_results` |

The seven new tests pass with normally composed components (durable and
non-durable matrices where applicable). A negative control that removed the
`AiTool` invocation site's own deadline selection failed the zero-budget test
with `unexpected child request 0`; the production selection was then restored.
The tests still use the existing private emitter harness because validation and
direct-support gates intentionally continue to reject Agent/Embed timeout syntax.
The full feature-gated compiler library suite passed 682 tests, the public AI
execution selection passed 19 tests, and feature-gated all-target Clippy passed.
Commands and skipped checks are in the implementation record.

The same inventory found additional public-enablement work in
`manifest::ai_agent_memory_provider` / `ai_agent_mcp_edges`: they extract provider
identity/connections but do not carry the referenced Agent timeout, and generated
`memory.load`, `memory.save` and `agent.tool.mcp` entries set `timeout: None`.
Those metadata and invocation paths require explicit budgets and tests. The
Agent-tool fix does not establish those contracts, complete G5, or justify
removing E128. Public validation, public component import inference and supported
export modes also need to be tested together when that gate is removed.
