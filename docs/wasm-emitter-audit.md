# WASM emitter audit

Audited 2026-09-05 against `cdcf9ee4ee0e5f28c0600b524c4984f89cbfe700`.
Scope: DSL validation, direct-WASM manifest/planning/lowering, JSON stdlib,
and durable suspend/resume through the production invoke ABI.

**Update 2026-09-06:** AUDIT-01 is committed as `2b6bf542` and AUDIT-02 as
`d787556e`, and AUDIT-03 as `a0629d99`. AUDIT-04 is fixed in the audit
worktree. AUDIT-05 through AUDIT-07 remain open. See the verification record for checks and limitations.

[Open the interactive pattern guide](wasm-emitter-patterns.html) to compare tested
controls, recorded failures, and proposed fixes with step-through diagrams and
exportable example DSL. The guide is a standalone, offline HTML/CSS/JS page;
its traces illustrate the audit evidence and do not run WASM.

Seven findings are documented below. The accompanying **46 audit tests** now
include **39 passing tests** and **7 known-defect regressions**. There are also
**29 passing unit tests**: 8 graph-analysis tests for AUDIT-01, 6 arena tests for
AUDIT-02, 7 identity tests and 1 compiler-version test for AUDIT-03, and 6 scoped
configuration tests and 1 compiler-version test for AUDIT-04. Two remaining
regressions execute composed WASM; five exercise validation or compilation
natively. The original AUDIT-01 through AUDIT-04 regressions now run normally;
their ignores were removed after the fixes.

The known-defect tests assert the **desired correct behavior** and currently fail.
They carry explicit `#[ignore = "AUDIT-XX: ..."]` reasons so normal CI stays green
while these fixes remain outstanding. They are not passing tests that enshrine
broken behavior. Remove each ignore when its fix lands. A fix should also run
that finding's controls, not just the previously failing case.

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

# Passing controls; known defects are reported as ignored:
RUSTC_WRAPPER= cargo test -p runtara-workflows --test wasm_emitter_audit
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit

# Explicit defect reproductions. BOTH commands currently exit nonzero:
RUSTC_WRAPPER= cargo test -p runtara-workflows --test wasm_emitter_audit -- --ignored --nocapture
RUSTC_WRAPPER= cargo test -p runtara-workflows --features direct-wasm-integration-tests --test direct_wasm_execute wasm_emitter_audit -- --ignored --nocapture

# One finding, including its controls and known defects:
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

A While with timeout 1,000 ms contains a Delay of 60,000 ms. A clock-controlled
invoke at time 1,000,000 parks until 1,060,000. Resuming at 1,060,001 completes
successfully instead of failing the While timeout.

The timeout is stored only in a WASM local and recomputed as now+timeout on replay.
It also does not constrain the returned wake to the enclosing deadline. Split
uses the same local-deadline pattern and reproduces the same failure in compiled
WASM. Both loop types complete normally when the delay is only 500 ms.
The loop iteration-limit exit precedes the next timeout check, so final-body
overruns also need coverage.

Source: [While deadline lowering](../crates/runtara-workflows/src/direct_wasm/compile/while_loop.rs), [Split deadline lowering](../crates/runtara-workflows/src/direct_wasm/compile/split.rs).

Fix direction: checkpoint the absolute deadline, restore it on replay, propagate
it into child suspension wakes, and check it before successful completion.

Tests:

| Test | Status on audited code |
| --- | --- |
| [`audit_05_while_completes_when_resume_is_within_timeout`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs) | Passing control |
| [`audit_05_split_completes_when_resume_is_within_timeout`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs) | Passing control |
| [`audit_05_while_timeout_survives_suspend_resume`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs) | Known defect; ignored by default, fails when selected |
| [`audit_05_split_timeout_survives_suspend_resume`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs) | Known defect; ignored by default, fails when selected |

<a id="audit-06"></a>

## AUDIT-06 · P2 — maximum retry count overflows in the compiler

An Agent with maxRetries 4294967295 parses and passes the support gate. Compiling
it in the debug/test profile panics at `max_retries + 1`. The EmbedWorkflow
reproduction also panics at its corresponding expression. Both compilers accept
0, 1 and u32::MAX - 1 in the boundary controls. Release behavior was not executed;
this boundary lacks a checked arithmetic/validation contract.

Source: [Agent retry arithmetic](../crates/runtara-workflows/src/direct_wasm/compile/agent_retry.rs) (`emit_agent_retry_delay`), [Embed retry arithmetic](../crates/runtara-workflows/src/direct_wasm/compile/embed_retry.rs) (`emit_embed_retry_delay`), [configuration validation](../crates/runtara-workflows/src/validation.rs).

Fix direction: bound accepted retries and use checked arithmetic that returns a
structured compile error. Cover u32::MAX and nearby limits.

Tests:

| Test | Status on audited code |
| --- | --- |
| [`audit_06_agent_retry_boundaries_below_overflow_compile`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Passing control |
| [`audit_06_embed_retry_boundaries_below_overflow_compile`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Passing control |
| [`audit_06_agent_retry_overflow_returns_compile_error`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Known defect; ignored by default, fails when selected |
| [`audit_06_embed_retry_overflow_returns_compile_error`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Known defect; ignored by default, fails when selected |

<a id="audit-07"></a>

## AUDIT-07 · P2 — step map keys and inner IDs can disagree

`entryPoint:"finish", steps:{finish:{id:"different",stepType:"Finish",...}}`
passes validation and the support gate, then compilation fails with
`missing direct entry step 'finish'`.

The validation reproductions cover a root mismatch, a nested mismatch, and two
map entries with the same inner ID. All three currently pass validation. A
matching-key control validates and compiles.

Validation resolves graph identity using map keys. Manifest construction discards
those keys and uses each step's inner id. Collisions between inner IDs can therefore
also create inconsistent graph identity.

Source: [graph validation](../crates/runtara-workflows/src/validation.rs) (`validate_graph_structure`), [manifest construction](../crates/runtara-workflows/src/direct_wasm/manifest.rs) (`graph_manifest`).

Fix direction: validate key==id recursively and enforce uniqueness within each
graph, or establish one canonical representation before graph analysis.

Tests:

| Test | Status on audited code |
| --- | --- |
| [`audit_07_matching_step_keys_validate_and_compile`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Passing control |
| [`audit_07_root_key_id_mismatch_is_rejected`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Known defect; ignored by default, fails when selected |
| [`audit_07_nested_key_id_mismatch_is_rejected`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Known defect; ignored by default, fails when selected |
| [`audit_07_duplicate_inner_ids_are_rejected`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Known defect; ignored by default, fails when selected |

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
- AUDIT-05: enclosing-deadline wake clamping and timeout overrun in the final iteration without suspension.
- AUDIT-06: release-profile behavior and other arithmetic limits in timeout/backoff lowering.

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
  Clippy checks passed. AUDIT-04 is left uncommitted for review.
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
