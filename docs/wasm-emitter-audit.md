# WASM emitter audit

Audited 2026-09-05 against `cdcf9ee4ee0e5f28c0600b524c4984f89cbfe700`.
Scope: DSL validation, direct-WASM manifest/planning/lowering, JSON stdlib,
and durable suspend/resume through the production invoke ABI.

**Update 2026-09-06:** AUDIT-01 is committed as `2b6bf542`. AUDIT-02 is also
fixed and verified in the audit worktree. AUDIT-03 through AUDIT-07 remain open.

[Open the interactive pattern guide](wasm-emitter-patterns.html) to compare tested
controls, recorded failures, and proposed fixes with step-through diagrams and
exportable example DSL. The guide is a standalone, offline HTML/CSS/JS page;
its traces illustrate the audit evidence and do not run WASM.

Seven findings are documented below. The accompanying **38 audit tests** now
include **28 passing tests** and **10 known-defect regressions**. There are also
**8 passing graph-analysis unit tests** for AUDIT-01 and **6 arena unit tests**
for AUDIT-02. Three remaining regressions
execute composed WASM; seven exercise validation, compilation, or manifest/stdlib
behavior natively. The original AUDIT-01 and AUDIT-02 regressions now run normally; their ignores
were removed after the fixes.

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

Two successive While bodies each contain a locally named `wait`, both at index 0.
First invoke suspends for `.../wait/[0]`. Delivering that signal causes the next
invoke to complete both loops: the second wait never requests a separate response.

Loop ancestry includes numeric indices but omits loop step identities. The
distinct `_scope_id` values do not participate in signal-key generation. Agent
and Split cache-key builders have the analogous omission, so they also warrant
replay regression coverage; actual agent-cache reuse was not executed in this audit.

Source: [stdlib key builders](../crates/runtara-workflow-stdlib/src/direct_json.rs) (`wait_signal_id`, `agent_cache_key`, `split_cache_key`, `split_iteration_variables`, `while_iteration_variables`).

Fix direction: include a collision-free structural path of loop IDs and indices
in every durable/signal key. Handle compatibility with existing parked instances.

Tests:

| Test | Status on audited code |
| --- | --- |
| [`audit_03_distinct_wait_ids_suspend_independently_and_replay_stably`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs) | Passing control |
| [`audit_03_same_local_wait_id_suspends_independently`](../crates/runtara-workflows/tests/wasm_emitter_audit/execution.rs) | Known defect; ignored by default, fails when selected |

<a id="audit-04"></a>

## AUDIT-04 · P2 — repeated step IDs select another scope's runtime configuration

Two sibling loop bodies define `wait` with timeoutMs 100 and 200. Validation and
the support gate accept them; both runtime lookups resolve to 100. This is
separate from durable-key collisions: fixing keys alone does not fix configuration.

The runtime recursively flattens graph steps into a single `BTreeMap<String,...>`
and retains the first definition via `or_insert_with`. Embedded child graphs
share this registry too; a separate manifest/stdlib test reproduces the same
timeout substitution across two embedded children. Wait timeout/action/schema
and other step-ID-based lookups cannot distinguish the definitions.

Source: [stdlib registry](../crates/runtara-workflow-stdlib/src/direct_json.rs) (`DirectJsonManifest::parse`, `collect_graph_manifest`, `wait_timeout_ms`).

Fix direction: use globally allocated manifest step identities or graph-qualified
lookup keys. Do not depend on graph-local user IDs being globally unique.

Tests:

| Test | Status on audited code |
| --- | --- |
| [`audit_04_distinct_loop_step_ids_keep_their_configuration`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Passing control |
| [`audit_04_duplicate_loop_step_ids_keep_their_configuration`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Known defect; ignored by default, fails when selected |
| [`audit_04_distinct_child_step_ids_keep_their_configuration`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Passing control |
| [`audit_04_duplicate_child_step_ids_keep_their_configuration`](../crates/runtara-workflows/tests/wasm_emitter_audit.rs) | Known defect; ignored by default, fails when selected |

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

- AUDIT-03: Agent/Split checkpoint collisions, nested loop paths, and compatibility with already parked instances.
- AUDIT-04: repeated step IDs with different types, wait actions, or response schemas.
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
