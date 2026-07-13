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
| [GAP-04](#gap-04) | P1 | AiAgent tool-loop has no per-turn checkpoint — crash re-runs and re-bills completed LLM turns; `durable` field doc promises otherwise | Checkpoint turn state per iteration | M | ai_agent_loop.rs + stdlib; strictly-better behavior | done |
| [GAP-05](#gap-05) | P1 | AiAgent tool-loop `onError` is dead — provider failures / max-iterations can't route to a handler | Provider/memory failures route via `error_plan`; tool errors still feed back to the LLM; exhaustion keeps complete-with-state | M | support gate + loop emitter; decorative onError edges start firing on recompile | done |
| [GAP-06](#gap-06) | P1 | Single-shot AiAgent `max_retries` hardcoded 0 | Add `maxRetries`/`retryDelay` to AiAgentConfig, wire existing retry machinery | S–M | DSL schema + manifest + frontend regen; default 0 keeps behavior | done |
| [GAP-07](#gap-07) | P1 | Gate/plan inconsistency: single-shot onError handler lowered live but never shape-checked by the gate | Shape-check handler in the gate for the chat-completion path | S | support.rs only | done |
| [GAP-08](#gap-08) | P2 | AiAgent tool-loop ignores `breakpoint` | Emit breakpoint pause at loop entry | S | ai_agent_loop.rs + plan field | done |
| [GAP-09](#gap-09) | P2 | `WaitForSignal.onWait` silently ignored when the step is an AiAgent tool | Validation warning W072 | S | Validator only | done |
| [GAP-10](#gap-10) | P2 | `Split.parallelism`/`sequential` accepted, execution always sequential | Validation warning W073 + doc; removed misleading W032 | S | Validator + docs; real parallelism is a separate epic | done |
| [GAP-11](#gap-11) | P2 | Stale diagnostics: AiAgent rejection says "single-shot only"; comments reference deleted fallback compiler; ErrorStep doc shows `${}` interpolation that doesn't exist | Rewrote messages/comments/doc example + pin test | S | Text only; actively misleading today | done |
| [GAP-12](#gap-12) | P2 | `workflow_has_side_effects` exported, uncalled, reads field names that no longer exist (always `false`) | Deleted fn + table + result field (no reader anywhere; catalog metadata too unreliable to fix against) | S | Public crate API removal | done |
| [GAP-13](#gap-13) | P3 | Conditioned normal-flow edges only allowed from Filter/GroupBy/Log/value-Switch sources | Extend `EdgeRoute` gate to Agent/Delay/WaitForSignal sources | M | Gate widening — only accepts more graphs | done |
| [GAP-14](#gap-14) | P3 | `onError` sources limited to Agent/EmbedWorkflow/Split/While | WaitForSignal timeout now routes to onError (structured WAIT_TIMEOUT envelope) | M | Gate widening | done |

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

**Update (2026-07-13) — configurable AI/agent timeouts.** Item 3 above (real
enforcement) is now partially delivered at the **outbound-HTTP layer** rather
than by guest preemption: a single `runtara_dsl::DEFAULT_STEP_TIMEOUT_MS`
(180000) is the default for AI/agent LLM calls, the `runtara-ai` providers apply
a per-request timeout (removing the prior 30s proxy floor), and codegen injects
`AgentStep.timeout` as `timeout_ms` into the capability input so capabilities
that accept one (e.g. the `http` agent, AI chat) bound their outbound HTTP call
via the proxy. A new `AiAgentConfig.turnTimeout` bounds each LLM brain turn
(per attempt) and is genuinely enforced; per-tool-call timeouts come from each
tool's own Agent-step `timeout`. W071 remains for the non-preemptible /
non-HTTP case; its wording now credits the HTTP-bound behavior and turnTimeout.

---

<a name="gap-04"></a>
## GAP-04 (P1) — AiAgent tool-loop has no per-turn durability

**Status: done** (2026-06-10)

**Problem.** `DirectRunPlan::AiAgentLoop` has no `durable_checkpoint` (`plan.rs:162-174`); the
loop only checkpoints memory load/save (`ai_agent_loop.rs:91-155`, `:440-585`). A crash/SIGTERM
mid-loop replays the workflow from the last checkpoint *before* the loop — completed LLM turns and
tool calls re-execute and re-bill. Worse, `AiAgentStep.durable`'s own doc
(`schema_types.rs:1191-1195`) promises "checkpoint on each tool call and LLM call inside this
agent's loop" — the DSL contract is already written; the loop doesn't deliver it.

**Fix plan.**
- [x] 1. Checkpoint unit: one snapshot per completed turn — LLM response (loop `state`), the
  turn's dispatched tool results (`pending`), and the monotonic tool-call counter — keyed
  `{step_id}.turn.{iteration}` scoped by `variables._loop_indices`. A completing turn snapshots
  with `complete: true` so even the final LLM call replays for free.
- [x] 2. Five new stdlib WIT functions (`ai-turn-cache-key`, `ai-turn-snapshot`,
  `ai-turn-snapshot-part`, `ai-turn-snapshot-tool-calls`, `ai-turn-snapshot-complete`) +
  `direct_json.rs` implementations; tool-call counter restores so WaitForSignal-tool signal ids
  stay resume-stable.
- [x] 3. `plan.rs`: `AiAgentLoop.durable_checkpoint` from the manifest agent entry's `durable`;
  dispatcher threads it through.
- [x] 4. `ai_agent_loop.rs`: per-turn lookup at iteration top (hit → restore
  state/pending/counter; completed-turn hit → exit loop with restored state) and snapshot save at
  both turn exits (tools-done and completion). `durable: false` skips everything.
- [x] 5. `AiAgentStep.durable` doc now describes the delivered behavior (it previously promised
  checkpoints the loop didn't do); phase12 plan's "durability hardening pending" closed.

**Test coverage delivered.**
- [x] Stdlib unit tests: `ai_turn_snapshot_round_trip_preserves_all_fields` (state/pending/
  counter/complete + invalid part), `ai_turn_cache_key_scopes_loop_indices`.
- [x] Execute e2e `..._replays_completed_turns_without_rebilling`: run 1 completes the tool-call
  turn and crashes on a turn-2 provider error (2 model calls, `ai.turn.1` checkpoint captured);
  run 2 preloads the turn checkpoints and completes with **exactly one** model call, and that
  call's request body still carries turn 1's tool result (the restored conversation). This is the
  crash-replay scenario, expressed via checkpoint preloading instead of SIGTERM — same replay
  path, deterministic.
- [x] Execute e2e `..._non_durable_skips_turn_checkpoints`: `durable: false` writes no
  `ai.turn.*` checkpoints and still completes.
- [x] Full suites: stdlib (125), workflows lib (450), execute (37, three consecutive clean runs).

---

<a name="gap-05"></a>
## GAP-05 (P1) — AiAgent tool-loop `onError` is dead

**Status: done** (2026-06-10)

**Problem.** The support gate treats AiAgent `onError` edges as inert and marks the handler
subgraph "dead, any shape allowed" (`support.rs:748-760`); `AiAgentLoop` has no `error_plan`
(`plan.rs:162-174`). Tool failures feed back to the LLM (correct, keep), but **loop-level**
failures — chat-turn capability/provider failure, memory load/save failure, max-iterations
exhaustion — terminate the workflow with no routing even when the user drew an onError edge.

**Fix plan.**
- [x] 0. Semantics decision: chat-turn (provider) failures and memory load/save failures route to
  the handler; individual TOOL failures stay unchanged (fed back to the LLM as the tool result).
  **Max-iterations exhaustion deliberately keeps its established contract** — the loop completes
  with the current state (no synthetic AI_MAX_ITERATIONS error). Rationale: exhaustion is a
  bounded-completion semantic today, not a failure; forking it on handler presence would make the
  same workflow complete or fail depending on an unrelated edge. Revisit only with a real demand
  signal.
- [x] 1. `support.rs`: AiAgent onError handlers are shape-checked on BOTH paths; the inert
  special case, `mark_dead_subgraph_reachable` usage for AiAgent, and the per-edge skip are gone
  (`ai_agent_is_single_shot` deleted as now-unused). `on_error_route_shape_supported` accepts all
  AiAgent sources.
- [x] 2. `plan.rs`: `AiAgentLoop.error_plan` built via `on_error_plan` like the single-shot arm.
- [x] 3. `ai_agent_loop.rs`: the chat-turn invoke and memory load/summarize/save invokes pass
  `error_plan` + failure/handled targets into the standard `emit_agent_invoke_error_branch`
  machinery (chat-turn site nests targets by 2 for the $outer/$turn blocks). `steps.__error.*`
  resolves in handlers.
- [x] 4. Behavior change managed: **no templateMajor bump** — handlers go live per-workflow on
  recompile; release-note it. Previously-accepted graphs with malformed decorative loop handlers
  now fail at the gate with a per-step report (intended).

**Test coverage delivered.**
- [x] Gate tests updated: `tool_loop_ai_agent_malformed_on_error_handler_is_rejected` (was the
  "inert any-shape passes" test) + new `tool_loop_ai_agent_well_formed_on_error_is_supported`.
- [x] Execute e2e `..._loop_provider_error_routes_to_on_error`: a stubbed provider 500 inside the
  loop routes to the handler Finish, which completes the workflow reading
  `steps.__error.code == "AI_TURN_COMPLETION_FAILED"`.
- [x] Execute e2e `..._loop_tool_error_feeds_back_not_on_error`: an unknown-capability TOOL
  failure feeds the error envelope back to the model (visible in the second request body), the
  loop recovers, the NORMAL finish runs — onError untouched.
- [x] Full suites: workflows lib (451), execute (39).

---

<a name="gap-06"></a>
## GAP-06 (P1) — Single-shot AiAgent cannot retry

**Status: done** (2026-06-10)

**Problem.** The single-shot path reuses the Agent emitter with `max_retries=0, retry_delay_ms=0`
hardcoded (`dispatcher.rs:687-689`). A transient provider 429/500 fails the step immediately;
`AiAgentConfig` has no retry fields to set.

**Fix plan.**
- [x] 1. `schema_types.rs`: `maxRetries`/`retryDelay` added to `AiAgentConfig`, **default 0** —
  LLM retries re-bill and are opt-in (doc comment says so).
- [x] 2. `manifest.rs`: both threaded into the AiAgent `ai-tools` agent entry.
- [x] 3. `plan.rs` carries `max_retries`/`retry_delay_ms` on `DirectRunPlan::AiAgent`;
  `dispatcher.rs` passes them to `emit_agent_plan` instead of the hardcoded zeros (chat-completion
  path reuses the full Agent retry machinery, incl. transient/permanent classification and durable
  retry sleeps). Per-turn loop retry deferred until GAP-04 (noted in dispatcher comment).
- [x] 4. `regen-frontend-api` run (offline `dump_openapi` variant) — `RuntaraRuntimeApi.ts` now
  carries the fields; frontend `tsc --noEmit` clean. Step-inspector UI wiring is generated-types
  driven; no manual frontend edits in scope.
- [x] 5. W030/W031 retry-hygiene warnings extended to AiAgent configs in `validate_configuration`.

**Test coverage delivered.**
- [x] Manifest unit test `ai_agent_manifest_threads_retry_config` (also covers DSL deserialization
  of the new camelCase fields end to end).
- [x] **New hermetic LLM stub in the execute harness**: `RUNTARA_HTTP_PROXY_URL` points the
  workflow's `call_agent()` proxy at the mock server; scripted `{status, headers, body}` envelopes
  are served per model call and request envelopes are recorded on `CapturedRun.llm_requests`.
- [x] Execute e2e: `..._completes_against_stub` (1 call, output threaded),
  `..._retries_transient_provider_errors` (500, 500, success with maxRetries:3 → completes with
  exactly 3 calls), `..._default_does_not_retry` (default fails on first 500 with exactly 1 call).
- [x] Full suites: workflows lib (450), execute (33).

---

<a name="gap-07"></a>
## GAP-07 (P1) — Gate/plan inconsistency on single-shot onError handlers

**Status: done** (2026-06-10)

**Problem.** The gate marks every AiAgent onError handler dead and skips shape-checking it
(`support.rs:748-760`), but the plan lowers the handler **live** for chat-completion steps
(`plan.rs:789-811`). A malformed handler subgraph passes the gate and only fails at plan build —
a raw `DirectCompileError` instead of a per-step support report (exactly the cascade-style
misdiagnosis the gate exists to prevent).

**Fix plan.**
- [x] 1. `support.rs`: new `ai_agent_is_single_shot` classifier (no edges labelled other than
  next/onError — mirrors the manifest's chat-completion selection). Single-shot AiAgent now runs
  the standard `on_error_supported_or_inert` walk; `on_error_route_shape_supported` accepts
  single-shot AiAgent sources; the per-edge shape-check skip applies only to tool-loop AiAgent.
  Tool-loop keeps inert handling until GAP-05.
- [x] 2. Comment block corrected under GAP-11 (cross-referenced, not duplicated).

**Test coverage delivered.**
- [x] Gate unit tests: `single_shot_ai_agent_well_formed_on_error_is_supported`,
  `single_shot_ai_agent_malformed_on_error_handler_is_rejected_at_gate`,
  `single_shot_ai_agent_two_default_on_error_edges_rejected` (error-handler-edge feature),
  `tool_loop_ai_agent_on_error_stays_inert_any_shape`.
- [x] Regression: full gate suite (86) green — all existing AiAgent fixtures still supported.
- [x] E2E (compile tier): `direct_wasm_compile_single_shot_ai_agent_gate_checks_on_error_handler`
  — the malformed-handler graph fails at the gate with the Unsupported report through the real
  `compile_direct_workflow_composed` path, and the well-formed one compiles AND composes to a
  non-empty `workflow.wasm`.

---

<a name="gap-08"></a>
## GAP-08 (P2) — AiAgent tool-loop ignores `breakpoint`

**Status: done** (2026-06-10)

**Problem.** `AiAgentStep.breakpoint` exists (`schema_types.rs:1188-1189`) and works for
single-shot (`plan.rs:155`, `agent.rs:93-105`), but `AiAgentLoop` has no breakpoint field — debug
mode cannot pause before a tool-loop step.

**Fix plan.**
- [x] 1. `plan.rs`: `breakpoint: bool` on `AiAgentLoop`, populated via `step_breakpoint_enabled`;
  `compile/tests.rs` breakpoint helper now includes the variant.
- [x] 2. `ai_agent_loop.rs`: `emit_step_breakpoint` at loop entry — before memory load and before
  the first LLM call, matching every other step's "pauses before this step" contract.

**Test coverage delivered.**
- [x] Execute e2e `..._breakpoint_pauses_before_first_llm_call`: DEBUG_MODE=true run exits cleanly
  with no /completed, no /failed, **zero** LLM calls (empty stub script would fail loudly on any
  call), the `breakpoint::ai` checkpoint stored, and the `breakpoint_hit` event emitted.
- [x] Execute e2e `..._breakpoint_resumes_with_checkpoint`: preloading the breakpoint-hit
  checkpoint short-circuits the pause and the tool loop runs to completion — one tool-call turn
  dispatched through the real utils component, then a completing turn (exactly 2 model calls).
- [x] Harness gained `extra_env` support (DEBUG_MODE knob). Full suites: lib (450), execute (35).

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

**Status: done** (2026-06-10)

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
- [x] 1. AiAgent rejection reason now states the actual requirements (config present; mcp.* edges
  target `mcp` Agent steps; memory edge ↔ config.memory agree; tool edges target Agent /
  compilable EmbedWorkflow / WaitForSignal); baseline doc comment rewritten to match.
- [x] 2–3. Corrected the stale fallback-compiler comments in `error.rs` and
  `direct_wasm/compile.rs` (module header + "opt-in" fn doc), and rewrote the AiAgent
  inert-onError comment in `support.rs` to state the real gate/plan split (single-shot handler IS
  lowered live → GAP-07; tool-loop handler dead → GAP-05).
- [x] 4. ErrorStep doc example fixed: static message + `context` mapping; also fixed the
  invalid `"category": "business"` in the same example (only `transient`/`permanent` exist).

**Test coverage delivered.**
- [x] New pin test `ai_agent_rejection_reason_names_actual_requirements` — asserts the new reason
  fires through the real gate and the stale "single-shot completions only" text cannot resurface.
- [x] `cargo doc`: the one link warning introduced by the rewrite fixed; remaining warnings
  pre-date this change. Full gate suite (82) green.

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

**Status: done** (2026-06-10)

**Problem.** `edge_condition_route_shape_supported` (`support.rs:1138-1176`) only accepts
conditioned normal-flow edges from Filter, GroupBy, Log, and value-Switch sources. A conditioned
edge off an Agent step ("route on the response payload without an extra Conditional") is rejected
with `edge-condition`, although the `EdgeRoute` plan machinery is source-agnostic.

**Fix plan.**
- [x] 1. `edge_condition_route_shape_supported` now accepts `Step::Agent` (baseline-gated),
  `Step::Delay`, and `Step::WaitForSignal` sources; shape rules unchanged (≥1 conditioned edge,
  exactly one unconditioned default, normal labels only). Split/While/EmbedWorkflow stay excluded
  (their successor handling owns next/error-plan interplay), documented in the match comment.
- [x] 2. Verified the `EdgeRoute` plan lowering is source-agnostic — no plan/dispatcher changes
  needed; the per-edge gate was the only restriction.
- [x] 3. Agent-source conditioned edges + onError on the SAME step verified end to end (failure
  routes to the handler; success takes the EdgeRoute).
- [x] 4. The gate's `edge-condition` reason text does not enumerate source types (only the shape
  rule), so no message change was needed.

**Test coverage delivered.**
- [x] Gate tests: `agent_source_conditioned_edges_are_supported` (fixture),
  `delay_and_wait_source_conditioned_edges_are_supported`,
  `agent_source_conditioned_edges_with_two_defaults_rejected` (shape rule still enforced).
- [x] New fixture `tests/fixtures/agent_edge_condition.json`: Agent whose output drives two
  conditioned branches (priority 10 vs 5) + default + an onError edge.
- [x] Execute e2e `..._agent_source_edge_conditions_route_on_agent_output`: three inputs through
  real WASM runs — `tier:vip` outranks `status:active` by priority, `active` routes, unknown
  status takes the default — all conditions reading `steps.echo.outputs.*` (the agent's own
  output, not raw input). The fixture's onError interplay was exercised for real during
  development: a mis-shaped agent input routed to the handler.
- [x] Full suites: workflows lib (454), execute (40).

---

<a name="gap-14"></a>
## GAP-14 (P3) — `onError` sources limited to Agent/EmbedWorkflow/Split/While

**Status: done** (2026-06-10) — activated and scoped to WaitForSignal, the one source with a
real use case (timeout → handler).

**Problem.** `on_error_route_shape_supported` (`support.rs:1178-1200`) only accepts onError edges
from Agent, EmbedWorkflow, Split, While. A Delay or WaitForSignal failure (e.g. wait timeout)
cannot route to a handler — the workflow fails outright. Workaround exists: wait timeouts can be
modeled with `timeout` + downstream checks, and GroupBy/Filter failures are deterministic data
errors better fixed upstream.

**Fix plan (delivered).**
- [x] 1. Scoped to **WaitForSignal timeout routing** (the plausible case named at parking time).
  GroupBy/Filter/Delay sources remain excluded — deterministic data errors are better fixed
  upstream, and no demand exists.
- [x] 2. Gate: the WaitForSignal walk consumes onError edges via `on_error_supported_or_inert`;
  `on_error_route_shape_supported` accepts WaitForSignal (and, from GAP-05, AiAgent) sources;
  both onError reason strings updated to enumerate the full source list.
- [x] 3. Plan/dispatcher: `WaitForSignal.error_plan` built via `on_error_plan` and threaded to
  `emit_wait_for_signal_plan`.
- [x] 4. Emitter: on deadline expiry with a handler present, the error routes through the shared
  `emit_agent_error_route_or_fail` machinery at the timeout site (the steps context is still the
  parent's there, so no capture block is needed; targets nest by 4 for the poll/timeout blocks).
- [x] 5. New stdlib WIT fn `wait-timeout-error-envelope` returns the structured
  `{code: WAIT_TIMEOUT, message, category: timeout, severity: error}` for routed handlers so
  `steps.__error.*` references resolve; the plain-string variant stays the /failed payload for
  parity with the generated path (pinned by the existing test).

**Test coverage delivered.**
- [x] Gate test `wait_for_signal_on_error_is_supported` (fixture).
- [x] New fixture `tests/fixtures/wait_timeout_on_error.json` (1ms deadline + handler reading
  `steps.__error.code` / `.category`).
- [x] Execute e2e `direct_wasm_execute_wait_timeout_routes_to_on_error`: a real WASM run expires
  the deadline and completes via the handler with `code == "WAIT_TIMEOUT"`,
  `category == "timeout"`.
- [x] Full suites: stdlib (127), workflows lib (455), execute (41).

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
