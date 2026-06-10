# Direct Emitter DSL Gap Resolution Tracker

Baseline audit: **2026-06-10**, main @ `7c3dbb3c` (v8.0.11). Source inventory: every DSL element was
compared against the direct emitter (`crates/runtara-workflows/src/direct_wasm/`), the runtime stdlib
(`crates/runtara-workflow-stdlib/src/direct_json.rs`), and the validator
(`crates/runtara-workflows/src/validation.rs`).

Status values: `todo` · `in-progress` · `done` · `parked` (needs a product/design decision first).
Update the Status column and tick the checkboxes as work lands. Each gap has an ID (`GAP-NN`) —
reference it in commit messages.

Effort: **S** = hours · **M** = 1–3 days · **L** = week+ · **XL** = architectural epic.

## Summary

| ID | Prio | Gap | Fix (short) | Effort | Blast radius | Status |
|----|------|-----|-------------|--------|--------------|--------|
| [GAP-01](#gap-01) | P0 | `MATCH`/`SIMILARITY_GTE`/`COSINE_DISTANCE_LTE`/`L2_DISTANCE_LTE` silently evaluate `false` in workflow conditions | Validation error E027 + stdlib stub → `Err` | S | validation.rs + 1 stdlib arm; mis-using workflows stop validating (intended) | done |
| [GAP-02](#gap-02) | P0 | `Agent.compensation` accepted, saga never runs | Validation warning W070 + doc honesty; removed W060 which suggested the no-op | S | Validator only; warning, not error | done |
| [GAP-03](#gap-03) | P0 | `Agent.timeout` / `EmbedWorkflow.timeout` parsed, never enforced | Validation warning W071 + doc honesty | S | Validator only; real enforcement is a separate epic | done |
| [GAP-04](#gap-04) | P1 | AiAgent tool-loop has no per-turn checkpoint — crash re-runs and re-bills completed LLM turns; `durable` field doc promises otherwise | Checkpoint turn state per iteration | M | ai_agent_loop.rs + stdlib; strictly-better behavior | todo |
| [GAP-05](#gap-05) | P1 | AiAgent tool-loop `onError` is dead — provider failures / max-iterations can't route to a handler | Lower loop-level failures into `error_plan` | M | support gate + loop emitter; decorative onError edges start firing on recompile | todo |
| [GAP-06](#gap-06) | P1 | Single-shot AiAgent `max_retries` hardcoded 0 | Add `maxRetries`/`retryDelay` to AiAgentConfig, wire existing retry machinery | S–M | DSL schema + manifest + frontend regen; default 0 keeps behavior | todo |
| [GAP-07](#gap-07) | P1 | Gate/plan inconsistency: single-shot onError handler lowered live but never shape-checked by the gate | Shape-check handler in the gate for the chat-completion path | S | support.rs only | todo |
| [GAP-08](#gap-08) | P2 | AiAgent tool-loop ignores `breakpoint` | Emit breakpoint pause at loop entry | S | ai_agent_loop.rs + plan field | todo |
| [GAP-09](#gap-09) | P2 | `WaitForSignal.onWait` silently ignored when the step is an AiAgent tool | Validation warning W072 | S | Validator only | done |
| [GAP-10](#gap-10) | P2 | `Split.parallelism`/`sequential` accepted, execution always sequential | Validation warning W073 + doc; removed misleading W032 | S | Validator + docs; real parallelism is a separate epic | done |
| [GAP-11](#gap-11) | P2 | Stale diagnostics: AiAgent rejection says "single-shot only"; comments reference deleted fallback compiler; ErrorStep doc shows `${}` interpolation that doesn't exist | Rewrite messages/comments/doc example | S | Text only; actively misleading today | todo |
| [GAP-12](#gap-12) | P2 | `workflow_has_side_effects` exported, uncalled, reads field names that no longer exist (always `false`) | Deleted fn + table + result field (no reader anywhere; catalog metadata too unreliable to fix against) | S | Public crate API removal | done |
| [GAP-13](#gap-13) | P3 | Conditioned normal-flow edges only allowed from Filter/GroupBy/Log/value-Switch sources | Extend `EdgeRoute` gate to Agent/Delay/WaitForSignal sources | M | Gate widening — only accepts more graphs | todo |
| [GAP-14](#gap-14) | P3 | `onError` sources limited to Agent/EmbedWorkflow/Split/While | Extend to Delay/WaitForSignal if demand appears | M | Gate widening | parked |

### Cross-cutting facts (read before picking up any item)

- **Stdlib is compiled into each artifact.** `runtara-workflow-stdlib` is composed into every
  `workflow.wasm` at compile time. A stdlib behavior change (GAP-01, 04, 05) reaches an existing
  workflow only on its next recompile. If fleet-wide effect matters, bump
  `TEMPLATE_MAJOR_VERSION` (`crates/runtara-workflows/src/compile.rs`) so the image cache
  miss-fires on next deploy.
- **Stdlib changes need a component rebuild** before any execute-level test:
  `RUNTARA_ONLY_WORKFLOW_COMPONENTS=1 ./scripts/build-agent-components.sh`.
- **Test tiers** referenced below:
  - *Gate/plan unit tests*: inline `#[cfg(test)]` in `direct_wasm/support.rs` and
    `direct_wasm/compile/tests.rs`, fixtures in `crates/runtara-workflows/tests/fixtures/*.json`.
  - *Stdlib unit tests*: inline in `runtara-workflow-stdlib/src/direct_json.rs`
    (pattern: `<step>_debug_payloads_supported`, `error_event_builds_payload_and_failure_message`).
  - *Validation unit tests*: inline in `validation.rs`, asserting `display.contains("[E0xx]")`.
  - *Compile+execute tests*: `crates/runtara-workflows/tests/direct_wasm_execute.rs`, gated by
    `RUNTARA_RUN_DIRECT_WASM_E2E=1` (needs prebuilt shared components, `wac`, `wasmtime`).
  - *Full-stack e2e*: the `e2e-verify` flow — boot server + embedded WASM runner, compile,
    register, execute, assert observable behavior (events, outputs, instance state).
- **Free code ranges** as of baseline: validation errors E027+ (E001–E026, E050–E060, E070–E073
  in use); warnings W070+ (W003–W060 in use). Codes assigned below; adjust if something lands
  in between.

---

<a name="gap-01"></a>
## GAP-01 (P0) — Query-only condition operators silently evaluate `false`

**Status: done** (2026-06-10)

**Problem.** `ConditionOperator` includes `SimilarityGte`, `Match`, `CosineDistanceLte`,
`L2DistanceLte` (`runtara-dsl/src/schema_types.rs:1786-1798`). They are documented "server-side
only; valid inside object-model query conditions", but nothing stops their use in workflow
conditions (Conditional/While/Filter/edge/onError-edge). The direct stdlib stubs them to
`Ok(false)` (`direct_json.rs:3964`), so e.g. a Conditional using `MATCH` compiles, validates, and
silently always takes the false branch. No validator rejects them in workflow context (the only
`ConditionOperator` match in `validation.rs` is `validate_field_condition_operation`, which is the
object-model *field condition* path where these ops are legitimate).

**Fix plan.**
- [x] 1. `validation.rs`: added **E027** (`QueryOnlyConditionOperator`) — Phase 10.5
  `validate_condition_operators` walks Conditional/While/Filter step conditions and every
  `executionPlan[].condition` (incl. `onError` edges), recursing `ConditionArgument::Expression`
  nesting, Split/While subgraphs, and WaitForSignal `onWait` graphs.
- [x] 2. Walker does not descend into agent `inputMapping` values — proven by the
  `e027_not_raised_for_object_model_query_conditions` test.
- [x] 3. `direct_json.rs`: stub arm now returns `Err("condition operator '<op>' is only valid
  inside object-model query conditions…")`.
- [x] 4. No DSL schema change → no `regen-frontend-api` needed. UI condition-builder hiding left
  as an optional frontend follow-up.
- [x] 5. `TEMPLATE_MAJOR_VERSION` decision: **no bump**. Validation blocks all new saves; already
  compiled images pick up the loud-error behavior on their next recompile.

**Test coverage delivered.**
- [x] Validation unit tests across all contexts: Conditional, While, Filter, plain edge +
  onError edge (one test), Split-subgraph + AND-nesting (one test) — `e027_*` in validation.rs.
- [x] Negative validation test: object-model query `inputMapping` with `SIMILARITY_GTE` stays
  clean.
- [x] Stdlib unit test `eval_condition_errors_on_query_only_operators` (all four ops → `Err`).
- [x] Execute-tier e2e (beyond plan): `direct_wasm_execute_query_only_condition_operator_fails_loudly`
  compiles a MATCH-Conditional graph directly (as a pre-E027 workflow would be) and asserts the
  real WASM run POSTs `/failed` with the operator message instead of completing via the false
  branch. Fixture: `tests/fixtures/conditional_query_only_operator.json`.

---

<a name="gap-02"></a>
## GAP-02 (P0) — `Agent.compensation` is a silent no-op (saga illusion)

**Status: done** (2026-06-10)

**Problem.** `AgentStep.compensation` / `CompensationConfig` (`schema_types.rs:443-446`,
`:380-405`) parse and deploy, but compensation is never emitted, never wired to the SDK, and never
triggered by the host — in the deleted generated path *or* the direct path
(`direct_wasm/support.rs:825-835`, `collect_agent_step_unsupported` comment). A user configuring
rollback gets none, with no signal.

**Fix plan.**
- [x] 1. `validation.rs`: **W070** (`CompensationNotEnforced`) warns whenever
  `AgentStep.compensation` is present, recursing Split/While subgraphs and WaitForSignal `onWait`
  graphs. Warning, not error: existing workflows keep validating.
- [x] 1b. (Found during implementation) Removed the old **W060** `MissingCompensation` warning and
  `CompensationSuggestion` — it *suggested adding* compensation to side-effecting steps, actively
  pushing authors into the no-op. Suggesting a feature that does nothing is worse than silence.
- [x] 2. `schema_types.rs`: `CompensationConfig` and `AgentStep.compensation` doc comments now
  state non-enforcement and point at `onError` routing.
- [ ] 3. Frontend follow-up (separate PR): remove/disable the compensation section in the step
  inspector, or render the W070 text inline. *(Not part of this commit.)*
- [x] 4. Real saga support stays out of scope; no design doc opened.

**Test coverage delivered.**
- [x] `test_compensation_present_warns_w070` (presence → W070 incl. display text),
  `test_no_compensation_no_w070_warning` (absence → silent, incl. on side-effecting capability).
- [x] `test_compensation_warns_w070_inside_split_subgraph` (nested case).
- [x] E2E: booted the full server stack and asserted
  `POST /api/runtime/workflows/graph/validate` returns the `[W070]` warning string for a graph
  with compensation, and no W060/W070 for one without.

---

<a name="gap-03"></a>
## GAP-03 (P0) — `Agent.timeout` / `EmbedWorkflow.timeout` parsed, never enforced

**Status: done** (2026-06-10)

**Problem.** Both fields deserialize and deploy but no deadline exists anywhere
(`support.rs:825-835` for Agent, `:947-955` for EmbedWorkflow). A synchronous
`capabilities.invoke` cannot be preempted in the component model, so per-step timeouts are
structurally unenforceable in-guest. Split/While/WaitForSignal timeouts *are* enforced (loop-edge
checks) — the inconsistency is invisible to users.

**Fix plan.**
- [x] 1. `validation.rs`: **W071** (`TimeoutNotEnforced`) warns when `AgentStep.timeout` or
  `EmbedWorkflowStep.timeout` is set, recursing Split/While subgraphs and `onWait` graphs.
- [x] 2. `schema_types.rs`: doc comments on both fields state non-enforcement and name the
  step types whose timeouts ARE enforced.
- [x] 3. Real enforcement (wasmtime epoch/deadline interruption in the runner +
  checkpoint-consistency analysis) stays a parked epic — not started under this tracker.
- [ ] 4. Frontend follow-up: surface W071 in the step inspector. *(Not part of this commit.)*

**Test coverage delivered.**
- [x] `test_agent_and_embed_timeout_warn_w071` (both step types + display text),
  `test_timeout_warns_w071_inside_while_subgraph` (nested).
- [x] `test_enforced_timeouts_do_not_warn_w071`: Split/While/WaitForSignal timeouts stay silent.
- [x] E2E: live server `POST /api/runtime/workflows/graph/validate` returns both W071 strings for
  an Agent+EmbedWorkflow-timeout graph and none for a Split-timeout graph.

---

<a name="gap-04"></a>
## GAP-04 (P1) — AiAgent tool-loop has no per-turn durability

**Status: todo**

**Problem.** `DirectRunPlan::AiAgentLoop` has no `durable_checkpoint` (`plan.rs:162-174`); the
loop only checkpoints memory load/save (`ai_agent_loop.rs:91-155`, `:440-585`). A crash/SIGTERM
mid-loop replays the workflow from the last checkpoint *before* the loop — completed LLM turns and
tool calls re-execute and re-bill. Worse, `AiAgentStep.durable`'s own doc
(`schema_types.rs:1191-1195`) promises "checkpoint on each tool call and LLM call inside this
agent's loop" — the DSL contract is already written; the loop doesn't deliver it.

**Fix plan.**
- [ ] 1. Design the checkpoint unit: one checkpoint per completed turn (LLM response + all tool
  results applied), keyed `"{step_id}.turn.{iteration}"` scoped with loop indices like existing
  Split/agent cache keys (`agent_cache_key` / `split_cache_key` precedents in `direct_json.rs`).
  State payload = the turn-state JSON the loop already threads (conversation messages, iteration
  counter, per-tool call counter).
- [ ] 2. `direct_json.rs`: add `ai_turn_cache_key(step_id, iteration, variables)` and
  state-envelope build/restore helpers (serialize is already JSON; restore must rehydrate the
  tool-call counter — WaitForSignal tool signal ids embed it and must stay resume-stable).
- [ ] 3. `manifest.rs` / `plan.rs`: thread `durable` (already extracted for single-shot at
  `manifest.rs:1111`) into `AiAgentLoop` as `durable_checkpoint`.
- [ ] 4. `ai_agent_loop.rs`: at loop top, checkpoint-read for the next iteration key — on hit,
  restore state and skip the LLM invoke + tool dispatch for that turn; after each completed turn,
  checkpoint-write. Honor `durable_checkpoint == false` by skipping both (current behavior).
- [ ] 5. Update `docs/wasm-direct-emitter.md` durability section; close the "durability hardening
  pending" line in `docs/wasm-direct-emitter-phase12-plan.md`.

**Test coverage required.**
- [ ] Plan unit test (`compile/tests.rs`): AiAgentLoop plan carries `durable_checkpoint` from the
  step/workflow flags (true / explicit false / non-durable workflow).
- [ ] Stdlib unit tests: `ai_turn_cache_key` determinism incl. loop indices; state round-trip
  (messages + iteration + tool-call counter survive serialize→restore).
- [ ] Execute test (`direct_wasm_execute.rs`, gated): 3-turn tool loop against a counting stub
  tool agent; kill the process after turn 2 (SIGTERM mid-run is an established harness pattern),
  re-run same instance, assert (a) final output correct, (b) the stub's invocation count proves
  turns 1–2 did **not** re-execute, (c) a WaitForSignal tool's signal id is identical across the
  resume.
- [ ] Full-stack e2e (`e2e-verify`): suspend/resume an AiAgent loop workflow; assert no duplicate
  per-turn debug events and exactly one `agent-debug` sequence per turn.

---

<a name="gap-05"></a>
## GAP-05 (P1) — AiAgent tool-loop `onError` is dead

**Status: todo**

**Problem.** The support gate treats AiAgent `onError` edges as inert and marks the handler
subgraph "dead, any shape allowed" (`support.rs:748-760`); `AiAgentLoop` has no `error_plan`
(`plan.rs:162-174`). Tool failures feed back to the LLM (correct, keep), but **loop-level**
failures — chat-turn capability/provider failure, memory load/save failure, max-iterations
exhaustion — terminate the workflow with no routing even when the user drew an onError edge.

**Fix plan.**
- [ ] 0. Semantics decision (blocks the rest): which failures route to onError?
  Proposal: provider/chat-turn invoke failure → route; memory load/save failure → route;
  max-iterations exhaustion → route with structured code `AI_MAX_ITERATIONS` (category
  `permanent`); individual tool errors → unchanged (fed back to LLM). Decision recorded: ______
- [ ] 1. `support.rs`: for AiAgent steps, replace the inert-edge special case with the standard
  `on_error_supported_or_inert` walk — add `Step::AiAgent` to the
  `on_error_route_shape_supported` source match; delete the `mark_dead_subgraph_reachable` call
  for AiAgent (`:755-760`). This also subsumes GAP-07 for the loop path.
- [ ] 2. `plan.rs`: add `error_plan: Option<DirectErrorRoutePlan>` to `AiAgentLoop`; build via
  `on_error_plan(...)` exactly as the single-shot arm does (`plan.rs:797-800`).
- [ ] 3. `ai_agent_loop.rs`: wrap the chat-turn invoke, memory load/save invokes, and the
  max-iterations exit in the standard failure-branch machinery
  (mirror `emit_agent_invoke_error_branch`, `agent.rs:278-291`), so `steps.__error.*` carries
  code/message/category for conditioned handlers.
- [ ] 4. Behavior change management: existing workflows with decorative AiAgent onError edges
  start catching on their next recompile. Release-note it; decide on `TEMPLATE_MAJOR_VERSION`
  bump (recommended: yes, pairs with GAP-04 landing). Decision: ______

**Test coverage required.**
- [ ] Gate unit tests: AiAgent with well-formed handler → supported; with >1 unconditioned
  onError edge → `error-handler-edge` unsupported feature; handler containing an unsupported
  shape → named rejection (no longer silently passes).
- [ ] Plan unit test: AiAgentLoop carries `error_plan`; conditioned handler branches ordered by
  priority.
- [ ] Execute tests: (a) stub provider whose chat-turn errors → handler runs, handler's Finish
  output reflects `steps.__error.code`; (b) loop exhausts maxIterations → routes with
  `AI_MAX_ITERATIONS` (if decided); (c) tool-agent error WITHOUT loop failure → still fed back to
  LLM, onError NOT taken (guard the unchanged semantics).
- [ ] Full-stack e2e: AiAgent → onError → Log → Finish; assert events show the handler scope.

---

<a name="gap-06"></a>
## GAP-06 (P1) — Single-shot AiAgent cannot retry

**Status: todo**

**Problem.** The single-shot path reuses the Agent emitter with `max_retries=0, retry_delay_ms=0`
hardcoded (`dispatcher.rs:687-689`). A transient provider 429/500 fails the step immediately;
`AiAgentConfig` has no retry fields to set.

**Fix plan.**
- [ ] 1. `schema_types.rs`: add `max_retries: Option<u32>` and `retry_delay: Option<u64>` to
  `AiAgentConfig` (mirror `AgentStep` field names/serde). **Default 0** — do not inherit Agent's
  default 3; silent retry of LLM calls re-bills tokens and must be opt-in.
- [ ] 2. `manifest.rs` AiAgent lowering (`:983-1035`): thread both into the agent entry (the
  manifest agent struct already carries retry for Agent steps).
- [ ] 3. `plan.rs`/`dispatcher.rs`: replace the hardcoded zeros with the manifest values for the
  chat-completion path. Scope note: per-turn retry inside the tool loop is **deferred** until
  GAP-04 lands (retry × replay interaction); record that in code comment.
- [ ] 4. Run `regen-frontend-api`; add the fields to the AiAgent step inspector.
- [ ] 5. Confirm existing retry-hygiene warnings (`W030`/`W031` HighRetryCount/LongRetryDelay)
  pick up the new fields; extend their walkers if they match on AgentStep only.

**Test coverage required.**
- [ ] DSL round-trip test: config with retries serializes/deserializes; absent fields default 0.
- [ ] Manifest unit test: retry values land on the AiAgent agent entry.
- [ ] Execute test: stub provider failing twice then succeeding — `maxRetries: 3` → completes,
  retry events visible with the configured delay key; `maxRetries: 0` (default) → fails
  immediately (regression guard on the default).
- [ ] Validation test: absurd retry values trigger the existing W030/W031 warnings for AiAgent.

---

<a name="gap-07"></a>
## GAP-07 (P1) — Gate/plan inconsistency on single-shot onError handlers

**Status: todo**

**Problem.** The gate marks every AiAgent onError handler dead and skips shape-checking it
(`support.rs:748-760`), but the plan lowers the handler **live** for chat-completion steps
(`plan.rs:789-811`). A malformed handler subgraph passes the gate and only fails at plan build —
a raw `DirectCompileError` instead of a per-step support report (exactly the cascade-style
misdiagnosis the gate exists to prevent).

**Fix plan.**
- [ ] 1. `support.rs` AiAgent arm: when the step has **no tool/mcp edges** (single-shot path —
  reuse the same edge classification as `supports_ai_agent_step_baseline`), run the standard
  `on_error_supported_or_inert` walk over its onError edges instead of
  `mark_dead_subgraph_reachable`. Keep inert handling for the tool-loop path *only until GAP-05
  lands*, then delete the special case entirely.
- [ ] 2. Correct the comment block at `support.rs:748-754` ("Direct does not lower the handler
  either" is false for single-shot) — coordinate with GAP-11 so it's not done twice.

**Test coverage required.**
- [ ] Gate unit tests: single-shot AiAgent + handler whose subgraph contains an unsupported shape
  → unsupported report names the handler step (not `execution-plan-routing` cascade); well-formed
  handler → supported; tool-loop AiAgent + any-shape handler → still supported (until GAP-05).
- [ ] Regression: all existing AiAgent fixtures still report supported.
- [ ] Compile test: the previously-passing malformed-handler graph now fails at the gate with the
  feature key, not at plan build.

---

<a name="gap-08"></a>
## GAP-08 (P2) — AiAgent tool-loop ignores `breakpoint`

**Status: todo**

**Problem.** `AiAgentStep.breakpoint` exists (`schema_types.rs:1188-1189`) and works for
single-shot (`plan.rs:155`, `agent.rs:93-105`), but `AiAgentLoop` has no breakpoint field — debug
mode cannot pause before a tool-loop step.

**Fix plan.**
- [ ] 1. `plan.rs`: add `breakpoint: bool` to `AiAgentLoop`, populate via
  `step_breakpoint_enabled` like every other arm.
- [ ] 2. `ai_agent_loop.rs`: emit the standard breakpoint pause (breakpoint_key/breakpoint_event
  stdlib calls — the AiAgent arms exist since c7068a06) at loop entry, before memory load.

**Test coverage required.**
- [ ] Plan unit test: breakpoint flag carried.
- [ ] Execute test: debug-mode run with breakpoint on the loop step pauses before the first LLM
  call (no provider invocation recorded), resume completes. Mirror the existing single-shot/Agent
  breakpoint test pattern.

---

<a name="gap-09"></a>
## GAP-09 (P2) — `onWait` ignored for WaitForSignal-as-AiAgent-tool

**Status: done** (2026-06-10)

**Problem.** A WaitForSignal step used as an AiAgent tool suspends and resumes correctly, but its
`on_wait` subgraph never runs (`plan.rs:865-874` builds `DirectAiToolPlan::Wait` without it; parity
with the generated path, documented at `support.rs:914-918`). A user who attached an approval-
request subgraph to the wait gets silence.

**Fix plan.**
- [x] 1. `validation.rs`: **W072** (`OnWaitIgnoredForAiAgentTool`) fires inside
  `validate_ai_agent_steps` when a tool-labelled edge (not next/onError/memory/mcp.*) targets a
  WaitForSignal step with `onWait` set.
- [x] 2. Doc note on `WaitForSignalStep.on_wait` in `schema_types.rs`.
- [x] 3. Running `onWait` for tool waits stays out of scope (demand-driven follow-up).

**Test coverage delivered.**
- [x] `test_wait_tool_with_on_wait_warns_w072` (incl. display text),
  `test_wait_tool_without_on_wait_no_w072`, `test_normal_flow_wait_with_on_wait_no_w072`.
- [x] E2E: live server `POST /api/runtime/workflows/graph/validate` returns the `[W072]` string
  for an AiAgent tool edge to a WaitForSignal-with-onWait.

---

<a name="gap-10"></a>
## GAP-10 (P2) — `Split.parallelism` / `sequential` are advisory-only

**Status: done** (2026-06-10)

**Problem.** `SplitConfig.parallelism`/`sequential` (`schema_types.rs:1986-1992`) deserialize and
even appear in debug-event payloads (`direct_json.rs:2738-2749`), but the emitted loop is strictly
sequential — single-threaded WASM. Users tuning `parallelism: 8` get nothing.

**Fix plan.**
- [x] 1. `validation.rs`: **W073** (`SplitParallelismIgnored`) fires for `parallelism` values
  other than 1 (0 = "unlimited" and >1 both promise concurrency that doesn't exist).
  Scope decision: `sequential: true` and `parallelism: 1` match actual behavior and stay silent —
  warning on them would only confuse.
- [x] 1b. (Found during implementation) Removed **W032** `HighParallelism` ("consider reducing for
  resource efficiency") — it implied parallelism works at all. W073 replaces it; no double-warn
  interaction left.
- [x] 2. `schema_types.rs` doc comments on both fields; sequential-execution note added to
  `docs/wasm-direct-emitter.md`.
- [x] 3. Real parallel Split (host-side fan-out execution) stays a separate, unstarted epic.

**Test coverage delivered.**
- [x] `test_split_parallelism_warns_w073` (parallelism 8 and 0, incl. display text),
  `test_split_parallelism_one_or_sequential_no_w073` (parallelism 1 / sequential:true / unset).
- [x] E2E: live server `POST /api/runtime/workflows/graph/validate` returns the `[W073]` string
  for `parallelism: 8`.

---

<a name="gap-11"></a>
## GAP-11 (P2) — Stale diagnostics and misleading docs

**Status: todo** *(background-task chip `task_f24a6ce6` already spawned with the full file list)*

**Problem.** Four text-level lies that mislead users/maintainers today:
1. `support.rs:1466-1471`: AiAgent rejection reason claims "single-shot completions only (no
   tool, memory, structured-output, or MCP edges)" — the loop, memory, MCP and structured output
   are all supported; a failing AiAgent gets a wrong explanation.
2. `support.rs:837-840`: baseline doc comment claims "exactly one Agent-capability tool" + "fall
   back to the generated Rust compiler".
3. `error.rs:6-11` and `direct_wasm/compile.rs:7`: comments describe falling back to the deleted
   generated compiler; `Unsupported` is a hard failure.
4. `schema_types.rs` ErrorStep doc example shows `"message": "Order total ${data.total}..."` —
   message is a verbatim string (`direct_json.rs:3395-3399`); no interpolation exists.

**Fix plan.**
- [ ] 1. Rewrite the AiAgent rejection reason to state actual requirements (config present; mcp.*
  edges target `mcp` Agent steps; memory edge ↔ config.memory agree; tool edges target
  Agent / lowerable EmbedWorkflow / WaitForSignal).
- [ ] 2–3. Correct the three stale comments (no behavior change).
- [ ] 4. Fix the ErrorStep example: static message + `context` mapping for dynamic values.

**Test coverage required.**
- [ ] Update the support.rs unit test that asserts the `ai-agent` feature reason string.
- [ ] `cargo doc -p runtara-dsl -p runtara-workflows` clean; no functional tests needed.

---

<a name="gap-12"></a>
## GAP-12 (P2) — `workflow_has_side_effects` is dead and broken

**Status: done** (2026-06-10)

**Problem.** Exported from `runtara-workflows` (`lib.rs:96`), zero callers in the workspace, and
it reads `operatorId`/`operationId` step keys (`compile.rs:72-78`) that the current schema renamed
to `agentId`/`capabilityId` years ago (`deny_unknown_fields` means the old keys can't even occur)
— so it always returns `false`.

**Fix plan.**
- [x] 1. Consumer check corrected an audit claim: there IS an internal caller
  (`compile_workflow_direct` populated `NativeCompilationResult.has_side_effects`), but **nothing
  reads that field** — not the server's compilation service, not the environment, not the CLI.
- [x] 1b. Evaluated the fix-instead-of-delete option (derive from the agent catalog's
  per-capability `hasSideEffects`): rejected — the real generated meta.json declares
  `hasSideEffects: false` for everything including `http:http-request`, so a catalog-driven
  version would also return false-everywhere until agent metadata is curated (separate effort).
- [x] 2. Deleted: `workflow_has_side_effects`, `SIDE_EFFECT_OPERATIONS`, the
  `NativeCompilationResult.has_side_effects` field, its computation at the call site, the
  `lib.rs` re-export, the three stale `operatorId`-fixture unit tests (which passed against the
  broken implementation while production always got `false`), and the two server test-fixture
  literals.

**Test coverage delivered.**
- [x] `cargo check` clean for runtara-workflows + runtara-server; grep guard: zero references
  remain in the workspace.
- [x] E2E: full gated `direct_wasm_execute` suite (29 tests — real compile→compose→wasmtime
  execution through `compile_workflow_direct`) green without the field; workflows lib suite (444)
  green.

---

<a name="gap-13"></a>
## GAP-13 (P3) — Conditioned normal-flow edges restricted to 4 source step types

**Status: todo**

**Problem.** `edge_condition_route_shape_supported` (`support.rs:1138-1176`) only accepts
conditioned normal-flow edges from Filter, GroupBy, Log, and value-Switch sources. A conditioned
edge off an Agent step ("route on the response payload without an extra Conditional") is rejected
with `edge-condition`, although the `EdgeRoute` plan machinery is source-agnostic.

**Fix plan.**
- [ ] 1. Extend the source match with `Step::Agent`, `Step::Delay`, `Step::WaitForSignal`
  (shape rules unchanged: ≥1 conditioned edge, exactly one unconditioned default, normal labels
  only). Hold Split/While/EmbedWorkflow back — their successor handling owns `next_plan`/
  `error_plan` interplay and needs its own analysis.
- [ ] 2. Verify `plan.rs` builds `EdgeRoute` for the new sources via
  `has_conditioned_normal_flow_edges` without special-casing (expected: yes; confirm with a
  structural test before touching anything).
- [ ] 3. Audit interaction: Agent source with BOTH conditioned normal edges and onError edges —
  ensure failure branches still take onError and successful output drives the EdgeRoute.
- [ ] 4. Keep the gate reason string in sync (it enumerates allowed sources).

**Test coverage required.**
- [ ] Gate unit tests: Agent/Delay/WaitForSignal sources with valid shape → supported; two
  defaults → `edge-condition` rejection; zero conditioned edges → unchanged behavior.
- [ ] New fixture `agent_edge_condition.json` (mirror `edge_condition_priority.json`): Agent whose
  output drives two conditioned branches + default, with edge `priority` ordering asserted.
- [ ] Execute test: correct branch taken for each input class, including the priority tie-break
  and the default path.
- [ ] Execute test: Agent with conditioned edges + onError — failure routes to handler, success
  routes through EdgeRoute (the GAP-13/onError interaction).
- [ ] Full-stack e2e: one scenario through `e2e-verify`.

---

<a name="gap-14"></a>
## GAP-14 (P3) — `onError` sources limited to Agent/EmbedWorkflow/Split/While

**Status: parked** *(needs a demand signal before investing)*

**Problem.** `on_error_route_shape_supported` (`support.rs:1178-1200`) only accepts onError edges
from Agent, EmbedWorkflow, Split, While. A Delay or WaitForSignal failure (e.g. wait timeout)
cannot route to a handler — the workflow fails outright. Workaround exists: wait timeouts can be
modeled with `timeout` + downstream checks, and GroupBy/Filter failures are deterministic data
errors better fixed upstream.

**Fix plan (when activated).**
- [ ] 1. Decide target sources from real demand (candidate: WaitForSignal timeout → onError is
  the only one with a plausible use case).
- [ ] 2. Extend the source match + per-step failure-target plumbing
  (`DirectFailureTarget`), mirroring the While onError lowering.
- [ ] 3. Update the gate reason strings (both `edge-condition` and `error-handler-edge` texts
  enumerate the allowed sources).

**Test coverage required (when activated).**
- [ ] Gate unit tests per new source; execute test modeled on the `wait_for_signal_direct_timeout`
  fixture but with an onError handler consuming `steps.__error.code == "WAIT_TIMEOUT"` (or the
  actual emitted code — pin it in the test).

---

## Out of scope — by design, do not "fix"

These fail loudly at compile time with per-step support reports and are structural invariants of
the tree-shaped lowering (core WASM has only structured control flow). Relaxing them means a
different compilation model, not a gap fix:

- Branch fan-out must re-converge or be the last step (no fan-out to two terminals — E073).
- No cycles in `executionPlan` — loops are expressed with While/Split.
- Conditional steps need exactly one `true` + one `false` edge; routing Switch needs exactly one
  edge per route label plus `default`.
- One entry point; every step reachable; every edge consumed (coverage invariant).
- EmbedWorkflow children are statically inlined at compile time (preloaded, acyclic closure,
  unique per call-site, child itself lowerable) — there is no runtime child resolution, and
  `childVersion: "latest"` pins at compile time.
- Nested `onError` inside an error-handler subtree stays dead (no recursive error handling).
