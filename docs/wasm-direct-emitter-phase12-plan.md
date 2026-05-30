# Phase 12: AiAgent Direct Lowering — Implementation Plan

Status: **Slices 0, 1, 2, 3 (memory), 4 (compaction), 5a (MCP tools) + multi-tool + 5b EmbedWorkflow-tool complete; 5b WaitForSignal-tool + durability pending.**

### Slice 5b (EmbedWorkflow tool) — DONE and e2e-verified

An AiAgent tool edge whose target is an EmbedWorkflow step with a preloaded child
graph runs the composed child once with the LLM tool arguments as its input data
(no input mapping, matching the generated `emit_embed_workflow_tool_arm`), feeding
the child's output back as the tool result. `DirectAiToolPlan` is now an enum
(`Agent { invoke } | Embed { child_plan }`); `emit_embed_workflow_tool_arm`
(embed_workflow.rs) reuses `emit_embed_workflow_child_attempt` (CHILD_DATA = tool
args → child variables → child run → output captured to the tool-result locals,
error wrapped on the child-error flag). The manifest already advertises such edges
and composes the child, so only the plan, loop dispatch, and support gate changed.
Verified by `direct_compile_supports_ai_agent_embed_workflow_tool_graph` (compile
+ WASM validation + Embed in plan) and the gated A/B test
`direct_wasm_matches_components_ai_agent_embed_workflow_tool` (runtime parity: the
mock LLM calls `get_weather`, both artifacts run the child and answer identically).

WaitForSignal-as-tool remains: it needs a tool-suffixed signal id
(`<instance>/<wf>/<step>.tool.<label>.<call#>`) built in a new stdlib helper
(WASM has no string ops), a per-tool-call counter, the `external_input_requested`
custom event, and the durable poll loop wired into the tool dispatch — a distinct
effort that also rebuilds the stdlib component. It still falls back to generated.

### Slice 5a (MCP synthetic tools) — DONE and e2e-verified

`mcp.<toolset>` edges lower at parity (gated A/B test
`direct_wasm_matches_components_ai_agent_mcp`). Each toolset advertises two
synthetic meta-tools (`<toolset>_search` → `mcp/mcp-tool-search`,
`<toolset>_invoke` → `mcp/mcp-tool-invoke`); these fit the existing static
tool-dispatch model (each is an agent invoke by tool index), so no new core-WASM
control flow was needed. The manifest advertises the two tool defs after the
Agent tools and adds two `agent.tool.mcp` provider entries per edge (named after
the synthetic tools); the plan resolves each advertised tool name to its Agent
edge or MCP provider, preserving order; a `chat-turn` `system_prompt_suffix`
field carries the MCP usage guide (matching the generated `mcp_prompt_addition`).
The A/B test drives `search → invoke → text` through a JSON-RPC MCP mock + a
connection-service mock in the harness.

Multi-tool dispatch (commit 82321f16) is done: the `chat-turn` capability
resolves each tool call's name to a `tool_index`, and the loop dispatches by
index over a `Vec<DirectAiToolPlan>`. Remaining AiAgent work: embed/wait tools
(Slice 5b) and per-turn durability/crash-resume hardening (Slice 6).

**onError is NOT an AiAgent feature** (corrected): the generated path's
`emit_step_execution` `can_have_on_error` covers only `Agent`/`Split`/
`EmbedWorkflow`/`While` — an AiAgent's LLM failure propagates fatally (an A/B
test confirmed the generated run fails on a 503 rather than routing to onError).
So the direct emitter must NOT route AiAgent failures to onError (it would
diverge); the loop correctly fails on a fatal chat-turn error, matching
generated. A failing *tool* is fed back to the model (Slice 6 / commit
58d17a4b), which is the real generated behavior.

### Slice 4 (memory compaction) — DONE and e2e-verified

Both compaction strategies lower at parity (gated A/B tests
`direct_wasm_matches_components_ai_agent_memory_compaction` and
`…_memory_summarize`). The generated path always compacts the conversation
before the memory save when memory is configured (default window 50), so the
Slice 3 memory lowering silently diverged for long conversations; this closes
that gap. **SlidingWindow** (default): the stdlib `ai-memory-compact-sliding`
drops the oldest messages so at most `max_messages` remain, invoked on the
final loop state before `ai-memory-save-input`. **Summarize**: the new
`ai-tools` `summarize-memory` capability LLM-summarizes the oldest excess into
a single `[Previous conversation summary]: …` message (the conditional + the
one LLM call live in the capability, so the direct loop stays branch-light);
`ai-summarize-input`/`ai-summarize-output` stdlib helpers build the call and
unwrap the compacted state. The A/B tests capture the object-model save payload
and assert the direct run persists the expected compacted conversation
(compaction is invisible in the completion payload).

### Slice 3 (conversation memory) — DONE and e2e-verified

An AiAgent with `config.memory` and a `memory`-labelled edge to an Agent step
now lowers at output parity (gated A/B test
`direct_wasm_matches_components_ai_agent_memory`). The manifest routes the
memory edge to the `object_model` provider (`load-memory` / `save-memory`),
adds the conversation mapping and load/save provider agent entries; the plan
carries a `DirectAiMemoryPlan`; `compile/ai_agent_loop.rs` resolves the
conversation id once, invokes `load-memory` and seeds the initial loop state
via `ai-memory-initial-state` before the turn loop, then `ai-memory-save-input`
+ `save-memory` after it. New stdlib helpers `ai-memory-initial-state` /
`ai-memory-save-input`. Intentional divergence from a generated defect: the
generated `codegen/ast/steps/ai_agent.rs` emitted the memory load/save end
debug events with the `data`/`input_mapping_json` arguments swapped (failed to
compile any memory AiAgent); fixed in the generated path so the A/B test can
verify both. The direct path was authored correctly and never inherited it.

### Slice 1 (tool loop) — DONE and e2e-verified

An AiAgent with a single Agent-capability tool now lowers to a validated WASM
component that runs the full tool loop at output parity with generated Rust
(gated A/B test `direct_wasm_matches_components_ai_agent_tool_loop`). The
`ai-agent-turn` capability does one turn in Rust; `compile/ai_agent_loop.rs`
emits the outer turn loop + inner tool-dispatch loop driven by the `ai-turn-*`
stdlib helpers, dispatching the tool agent with the LLM-provided args and
threading loop state between turns. Remaining tool-loop work folded into later
slices: **multiple tools** (name-matched dispatch — Slice 5), MCP/embed/wait
tools (Slice 5), tool-loop onError routing, and durability/checkpoint per turn
(Slice 6).

Original design (now implemented), kept for reference:
- **plan/manifest**: a `DirectRunPlan::AiAgentLoop` carrying the base
  chat-turn input mapping + a **tool table** — for each tool edge (label →
  target Agent step), `{name, agent_component_id, capability_id,
  input_mapping_id?}`. Build it from the AiAgent's labelled edges (excluding
  `next`/`onError`/`memory`/`mcp.*`).
- **stdlib helpers** (~6, mirror the `while-*` family):
  `ai-turn-input(base, state) -> turn_input` (merge mapping result + loop
  state); `ai-turn-is-complete(turn_out) -> bool`; `ai-turn-tool-count`,
  `ai-turn-tool-name(i)`, `ai-turn-tool-args(i) -> bytes`;
  `ai-turn-next-state(turn_out, pending_results) -> state`;
  `ai-turn-output(turn_out) -> step_output` (`{response, iterations, toolCalls}`).
- **core-WASM** (`compile/ai_agent_loop.rs`): outer `block`/`loop` (reuse the
  While Block+Loop + frame save/restore). Body: build turn input → invoke
  `ai-tools`/`chat-turn` (reuse `emit_agent_invoke`) → if complete, emit output
  + `Br outer`; else inner `block`/`loop` over the tool calls (reuse Split's
  count+index iteration): per tool, match `name` against the tool table and
  `emit_agent_invoke` the matching tool agent, collect the result into
  `pending_results`; then `Br turn`.
- **support**: accept AiAgent steps whose only labelled edges are
  Agent-capability tool edges (gate MCP/embed/wait tools + memory to later
  slices).
- **test**: gated A/B with the mock LLM returning a tool call on turn 1 then
  text on turn 2; use an in-WASM tool (e.g. `utils`/`return-input`) so no extra
  proxy mock is needed. Assert completion-payload parity (checkpoint traffic
  differs by design, as in Slice 0).

Slice 2 (structured output) is done and e2e-verified: AiAgent steps with an
`outputSchema` lower in direct mode; the `chat-completion` capability parses the
response as JSON (`structured_output`, string fallback) and `ai-agent-output`
uses it as the response value. Gated A/B test
`direct_wasm_matches_components_ai_agent_structured_output`. (Also fixed a latent
bug: the synthesized chat-completion input mapping must use snake_case keys —
`ChatCompletionInput` deserializes snake_case — or the prompts/schema silently
drop, which the deterministic mock had masked.)


Slice 0 (single-shot AiAgent) is done and e2e-verified: a single-shot AiAgent
compiles to a validated WASM component and runs at output parity with the
generated Rust path (gated A/B test
`direct_wasm_matches_components_ai_agent_single_shot`). One intentional
divergence: checkpoint traffic differs (generated uses a bespoke
`agent::<step>/llm/<iter>` LLM checkpoint; direct reuses the standard Agent
checkpoint `agent::ai-tools::chat-completion::<step>`), so the A/B test asserts
completion-payload parity, not checkpoint parity. Both are internally consistent
for crash/resume.

Decisions (resolved with the maintainer):
- The `chat-completion` capability is added to the **existing
  `runtara-agent-ai-tools`** component (not a new component).
- Build proceeds **Slice 0 end-to-end** (single-shot AiAgent at A/B parity).
  If the shared deterministic mock-LLM proxy proves infeasible, fall back to
  "direct behaves correctly" assertions (as done for Split onError) rather than
  blocking.

This plan is the concrete design for direct-WASM-emitter support of the
`AiAgent` step, the sole remaining unsupported step type. It is written against
the verified current state of the codebase (May 2026).

## Slice 0 progress (live)

Done:
- **`runtara_ai::run_completion`** + `CompletionInvokeRequest` (new
  `orchestration` module) — single source of truth for the loop's LLM call,
  identical to the generated `__ai_llm_durable` body.
- **`chat-completion` capability** on `runtara-agent-ai-tools` — calls
  `run_completion`, returns the raw assistant `choice` (+ usage). Builds as a
  `wasm32-wasip2` component with `runtara-ai` linked in. Agent count unchanged
  (new capability on an existing component, so no staged-component-count or
  compose-graph change). Committed.

Mock-proxy feasibility (the A/B linchpin) — **confirmed feasible**:
- The gated harness runs `wasmtime run --wasi http --wasi inherit-network` with
  a local server reached via `RUNTARA_HTTP_URL`/`RUNTARA_SERVER_ADDR` (runtime
  callbacks: load-input/complete/fail/events/checkpoints).
- `runtara-http::call_agent` routes outbound provider calls through
  `RUNTARA_HTTP_PROXY_URL` when set; the harness does NOT set it today.
- Plan: set `RUNTARA_HTTP_PROXY_URL` to the test server and extend its handler
  to recognize the proxy envelope and return a **canned chat-completion**
  response (parseable by `runtara-ai`'s openai/bedrock client). Both generated
  and direct paths hit the same mock → identical `choice` → A/B parity. Needs:
  (a) the runtara-http proxy request/response envelope shape, (b) a minimal
  OpenAI chat-completion response body.

Remaining Slice 0 steps (direct-lowering half):
1. **stdlib WIT + impl**: add `ai-agent-output(source, step-id, choice) ->
   steps` (extract final assistant text from the choice; build the
   `{response, iterations:1, toolCalls:[]}` envelope, wrapped as the step
   output context — matches generated `__step_output_envelope` for the
   single-shot case). Rebuild stdlib/runtime components
   (`RUNTARA_ONLY_WORKFLOW_COMPONENTS=1 ./scripts/build-agent-components.sh`).
2. **manifest.rs**: build a `DirectAgentManifest`-like entry for the AiAgent
   step that targets `ai_tools`/`chat-completion`, plus an input mapping that
   produces the chat-completion input (`{systemPrompt, userPrompt, model,
   temperature, maxTokens}`) from the AiAgentConfig MappingValues.
3. **plan.rs**: `DirectRunPlan::AiAgent { step_id, input_mapping_id,
   durable_checkpoint, next_plan, error_plan, ... }` (single-shot subset).
4. **static_data.rs**: AiAgent capability/connection string segments (reuse the
   agent segment machinery, since the target is the `ai_tools` agent).
5. **support.rs**: replace the `Step::AiAgent` hard rejection with
   `supports_ai_agent_step_baseline` accepting only the single-shot subset (no
   tools edges, no memory edge, no output schema, no compaction, default
   iterations) and rejecting everything else (clean Rust fallback).
6. **compile/ai_agent.rs** (new): `emit_ai_agent_plan` — apply input mapping →
   (durable: checkpoint) invoke `ai_tools`/`chat-completion` (reuse
   `emit_agent_invoke`) → `ai-agent-output` → build-source → next_plan; error
   capture via `emit_agent_error_route_or_fail`.
7. **dispatcher.rs / core_module.rs**: route `DirectRunPlan::AiAgent`; allocate
   any AiAgent-specific locals.
8. **Tests**: structural core-WASM test (compile a single-shot AiAgent fixture,
   assert it validates + invokes chat-completion); gated A/B test against the
   mock proxy (assert identical output, or "direct correct" if the shared mock
   proves impractical).

## 1. Goal and scope

Lower `AiAgent` workflows through the direct emitter at full parity with the
generated Rust path, **without linking LLM-provider logic into every
`workflow.wasm`** (the explicit Phase 12 objective and the standing TODO in
`runtara-workflow-stdlib/Cargo.toml`).

In scope (parity surface of the generated loop, per
`codegen/ast/steps/ai_agent.rs`):
- system/user prompt resolution, model/temperature/max_tokens config
- the agentic tool loop with `max_iterations` bound
- tool dispatch (Agent-capability tools, EmbedWorkflow tools, WaitForSignal
  tools, MCP synthetic `*_search`/`*_invoke` tools)
- conversation memory load/save via the `memory`-labelled edge
- structured output (`output_schema` → provider params; response JSON parse)
- compaction (SlidingWindow default; Summarize strategy)
- durability: per-LLM-call and per-tool-call checkpoints; breakpoint pause
- step output envelope `{response, iterations, toolCalls}`
- `step_debug_*` events for the step, tool calls, memory load/save, compaction

## 2. What is already behind a component boundary vs. linked in

Already behind the universal agent ABI (`runtara:agent@0.3.0/capabilities.invoke`),
reachable from direct mode today via `emit_agent_invoke`:
- **Tools** — every Agent-capability tool is a normal `invoke` call.
- **Memory** — `object_model` agent exposes `load-memory` / `save-memory`.
- **MCP** — `mcp` agent exposes `mcp-tool-search` / `mcp-tool-invoke`.
- **EmbedWorkflow / WaitForSignal tools** — already lowerable by the direct
  emitter (Phase 10/11 work).

Linked directly into `workflow.wasm` today (the **only** thing Phase 12 must
move):
- **`runtara-ai`** (re-exported as `stdlib::ai`): `CompletionModel`, the
  OpenAI/Bedrock providers, and the chat-completion request/response shape.
  HTTP egress already leaves the sandbox via `runtara-http` →
  `wasi:http/outgoing-handler` → the `RUNTARA_HTTP_PROXY_URL` proxy (which
  injects credentials / SigV4). So the *network path* is already correct; only
  the *code* is mislocated.
- The **orchestration loop** itself (emitted Rust manipulating
  `Vec<RigMessage>`).

## 3. Architecture decision

**Move the LLM completion call behind the existing agent `invoke` ABI** rather
than inventing a new WIT interface. Concretely:

1. Add a **`chat-completion` capability** to a provider-agnostic AI component.
   - Preferred: extend the existing `runtara-agent-ai-tools`
     (`runtara:agent-ai-tools@0.3.0`) with a new `chat-completion` capability,
     OR add a dedicated `runtara-agent-ai-completion` component if we want the
     loop-completion path isolated from the single-shot ai-tools capabilities.
   - The capability links `runtara-ai` internally and performs exactly the
     `create_completion_model_with_connection` + `structured_output_params` +
     `completion` sequence that `__ai_llm_durable` performs inline today.
   - Request JSON (input bytes): `{provider, model, systemPrompt, userPrompt,
     chatHistory, tools, temperature, maxTokens, outputSchema}` plus the
     standard `connection`.
   - Response JSON (output bytes): the serialized `OneOrMany<AssistantContent>`
     choice (and optional usage) — byte-identical to what
     `serde_json::to_value(&resp.choice)` produces today, so generated and
     direct send identical requests to the mock proxy (free A/B parity).

2. **Lower the orchestration loop in direct core-WASM** as control flow that
   calls: the new `chat-completion` capability (LLM), normal agent `invoke`
   (tools/memory/MCP), and a set of **new stdlib JSON helpers** for the
   conversation/loop semantics that today live as inline Rust.

This reuses: the agent-invoke ABI, `wac compose`, the durable
checkpoint/retry machinery (`__ai_llm_durable`/`__ai_tool_durable` map directly
onto the existing direct checkpoint+retry lowering), and the While/Split
reentrant-loop precedent for the iteration loop.

Rejected alternative: a bespoke `runtara:ai-provider` WIT interface. The agent
ABI (JSON `list<u8>` in/out) already carries everything; a new interface adds
compose/import plumbing for no benefit.

## 4. New component capability ABI

`chat-completion` on the AI component (JSON in/out, per agent ABI):

```
input  = {
  provider: "openai" | "bedrock",
  model: string?,
  systemPrompt: string,
  userPrompt: string,            // empty after iteration 1
  chatHistory: [Message],        // serialized rig Message list
  tools: [ToolDefinition],
  temperature: number?,
  maxTokens: integer?,
  outputSchema: object?          // JSON Schema; drives structured_output_params
}
output = { choice: OneOrMany<AssistantContent>, usage: {..}? }
error  = standard error-info (retryable/rate-limited classified as today)
```

The component body is ~the current `__ai_llm_durable` inner function, minus the
`#[resilient]` wrapper (durability moves to the direct core checkpoint around
the invoke, exactly like a normal durable Agent step).

## 5. New stdlib (`runtara:workflow-stdlib/json`) helpers

The conversation/loop semantics that are inline Rust today must become JSON
helpers (direct core only does control flow + JSON via stdlib). Proposed:

- `ai-init-loop(source) -> loop-state` — seed `{chatHistory:[], toolLog:[],
  iterations:0, toolCallCounter:0, finalResponse:null}`.
- `ai-build-completion-input(manifest-step-id, loop-state, system, user,
  tools, config) -> list<u8>` — assemble the `chat-completion` input bytes for
  the current iteration (user prompt only on iter 1; MCP system-prompt
  addition).
- `ai-parse-choice(loop-state, choice) -> {loop-state, has-tool-call,
  pending-tool-calls}` — split the choice into assistant text (→
  finalResponse) and tool calls; append assistant message; queue tool results.
- `ai-tool-call-input(tool-call, tool-table) -> {agent-id, capability-id,
  inputs, kind}` — resolve a tool call to a dispatch target (Agent / Embed /
  Wait / MCP-search / MCP-invoke / unknown).
- `ai-append-tool-result(loop-state, tool-call-id, result) -> loop-state`.
- `ai-memory-load-input(...) / ai-memory-sanitize(chatHistory) -> chatHistory`
  — the orphan tool-call / orphan tool-result sanitization.
- `ai-memory-save-input(loop-state, conversation-id) -> list<u8>`.
- `ai-compact(loop-state, strategy, max-messages) -> {loop-state, needs-llm,
  summarize-input?}` — SlidingWindow drain, or produce the Summarize LLM input;
  `ai-apply-summary(loop-state, summary) -> loop-state`.
- `ai-output-envelope(step-id, name, loop-state, has-output-schema) ->
  list<u8>` — `{response, iterations, toolCalls}` with response JSON-parse vs
  string fallback.
- `ai-loop-continue(loop-state, max-iterations) -> bool` — iteration bound +
  has-tool-call exit.

These mirror the existing `split-*` / `while-*` helper families and keep all
JSON shape decisions in one testable Rust location (`runtara-ai`/stdlib),
reused by both compilation paths.

## 6. Runtime ABI

**No new runtime functions required.** The loop reuses: `get-checkpoint`,
`checkpoint`, `durable-sleep[-checkpoint]`, `record-retry-attempt`,
`handle-checkpoint-signal`, `custom-event` (debug events), `breakpoint-pause`,
`check-signals`/`is-cancelled`. The per-LLM-call and per-tool-call checkpoints
use the same cache-key shapes the generated loop already uses
(`{base}/llm/{iter}`, `{base}::tool::{label}::{n}`, `{base}/memory_*`, etc.) —
these must be reproduced exactly for crash/resume parity.

## 7. Direct lowering design

- **plan.rs**: add `DirectRunPlan::AiAgent { step_id, ai_component_id,
  provider, model, ..config.., max_iterations, durable, breakpoint, tool_table,
  memory_plan, compaction, output_schema_id, next_plan, error_plan }`. The
  `tool_table` maps tool-name → dispatch target (reusing Agent/Embed/Wait plan
  fragments so tool bodies lower through existing emitters).
- **manifest.rs / static_data.rs**: serialize the AI config, tool table,
  output-schema JSON, MCP toolset ids, memory agent id/connection as static
  data segments + allocated ids (mirrors `DirectAgentManifest`).
- **support.rs**: replace the `Step::AiAgent` hard rejection with a baseline
  gate (`supports_ai_agent_step_baseline`) that accepts the lowerable subset
  and still rejects anything not yet covered (slice-gated; see §8).
- **compile/ai_agent.rs** (new): `emit_ai_agent_plan` — the orchestration loop:
  - allocate a new local band above index 63 for AI loop state pointers
    (loop-state ptr/len, choice ptr/len, has-tool-call flag, iteration counter,
    pending-tool-calls ptr/len). The While/Split loop locals (18..27) are reused
    where the tree structure guarantees no co-occurrence; dedicated locals for
    anything that must survive a nested tool body.
  - structure: `loop { ai-loop-continue? → break; build-completion-input;
    [checkpoint] invoke chat-completion [retry]; ai-parse-choice; for each
    tool-call: dispatch via existing agent/embed/wait emitter [checkpoint];
    ai-append-tool-result; if !has-tool-call break }`.
  - tool dispatch reuses `emit_agent_invoke` / embed / wait lowerers, wrapped in
    the per-tool checkpoint, exactly like the durable Agent path.
  - memory load before the loop, compaction + memory save after, each behind a
    checkpoint.
  - error capture + onError routing reuse `emit_agent_error_route_or_fail`
    (same `error-steps` + dispatch chain as Agent/Split/While onError).
- **dispatcher.rs**: route `DirectRunPlan::AiAgent` → `emit_ai_agent_plan`.
- **component.rs / compile.rs**: add the AI component to
  `DIRECT_SHARED_COMPONENT_REQUIREMENTS`, the `emit_wac` graph, the world WIT
  imports, the `-d` compose args, and the sidecar resolution — exactly the
  5-step "add a new component" sequence already used for stdlib/runtime/agents.

## 8. Incremental slices (MVP-first; each slice is shippable + tested)

1. ✅ **Slice 0 — component + MVP single-shot.** Add the `chat-completion`
   capability and a deterministic mock provider. Direct-lower the *no-tool,
   no-memory, no-structured-output, single-iteration* AiAgent: build input →
   checkpointed invoke → extract final text → output envelope. Gate everything
   else. Prove end-to-end + A/B parity on one fixture.
2. ✅ **Slice 1 — multi-iteration tool loop** (Agent-capability tools only).
   Iteration bound, tool dispatch + per-tool checkpoint, conversation append.
   (+ multi-tool name-matched dispatch.)
3. ✅ **Slice 2 — structured output** (`output_schema`).
4. ✅ **Slice 3 — memory** load/save (object_model edge).
5. ✅ **Slice 4 — compaction** (SlidingWindow + Summarize).
6. **Slice 5** — ✅ **5a MCP synthetic tools**; ⏳ 5b EmbedWorkflow/WaitForSignal
   tools.
7. **Slice 6 — durability hardening**: breakpoint pause/resume, crash/resume
   differential tests across LLM/tool checkpoints; retry parity. (NOT onError
   routing — that is not an AiAgent feature in the generated path; tool errors
   are fed back to the model, done in commit 58d17a4b.)

Each slice: ungate its subset in `support.rs`, add structural core-WASM test +
gated A/B execution test, fmt/clippy, doc update, commit. The support gate keeps
un-implemented sub-features rejected (fall back to Rust) until their slice
lands — no half-supported AiAgent ever compiles silently.

## 9. Mock-provider / test strategy

- The LLM call leaves via `RUNTARA_HTTP_PROXY_URL`. The existing E2E harness
  already mocks this proxy for HTTP agents. A **deterministic mock completion
  endpoint** (canned `choice` responses keyed by a hash of the request) drives
  both generated and direct artifacts identically.
- Because generated (`runtara-ai` inline) and direct (the `chat-completion`
  component, which *also* uses `runtara-ai`) emit byte-identical completion
  requests, the mock returns identical responses → true A/B parity on output,
  tool-call sequence, checkpoint traffic, and events.
- Test matrix per slice: single-shot; one-tool-call-then-finish;
  max-iterations-exhausted; structured-output (valid + invalid JSON fallback);
  memory load→save round-trip; compaction (over/under threshold; summarize
  success + degraded); MCP search+invoke; breakpoint pause→resume; crash after
  LLM checkpoint → resume; crash after tool checkpoint → resume.
- Reuse `direct_wasm_ab_execute.rs` harness + `assert_success_parity` (with
  volatile fields normalized, as the WaitForSignal/Agent tests already do).

## 10. Risks and open questions

- **Mock-provider determinism** is the linchpin of A/B. If the mock can't be
  shared by both paths, A/B degrades to "direct behaves correctly" assertions
  (as we did for Split onError). Confirm the harness can host a canned-LLM
  endpoint.
- **runtara-ai in a component**: the crate already targets `wasm32` with the
  `wasi` http backend, so compiling it inside an agent component should be
  mechanical — but the component must be added to `build-agent-components.sh`
  and the staged-component count assertions updated.
- **Loop-state as JSON** adds per-iteration serialize/deserialize overhead vs.
  the generated `Vec<RigMessage>`. Acceptable (matches how every other direct
  step already works); note for later perf review.
- **`chat-completion` vs `ai-tools`**: decide whether to extend `ai-tools` or
  add a dedicated `ai-completion` component. Dedicated is cleaner for isolating
  the loop path and the staged-component diff; extending reuses an existing
  component slot.
- **Compaction Summarize** issues a second LLM call mid-step with its own
  checkpoint key; ensure the mock + A/B cover it.

## 11. Effort estimate

Large, multi-commit, multi-session. Roughly: Slice 0 (component + ABI + MVP
loop + harness) is the heaviest single lift (new component, new stdlib helper
family scaffolding, new lowerer, mock provider). Slices 1–6 are incremental on
that foundation. Total comparable to Phases 9–11 combined.
