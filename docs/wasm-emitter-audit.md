# WASM emitter audit

Audited 2026-09-05 against `cdcf9ee4ee0e5f28c0600b524c4984f89cbfe700`.
Scope: DSL validation, direct-WASM manifest/planning/lowering, JSON stdlib,
and durable suspend/resume through the production invoke ABI.

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

Seven findings are documented below. All **77 audit tests pass**, with **none
ignored**. There are also **42 passing unit tests**: 8 graph-analysis tests for
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
