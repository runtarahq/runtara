# Workflow-as-Agent — design + build plan

_Produced via multi-agent mapping + design synthesis, verified against the feature branch (2026-07-15)._

**Slice a obstacle — CONFIRMED against source (2026-07-15), revises the design:** the design called slice a "a small delta over the lifecycle emit." It is not. `capabilities.invoke(capability-id, input, connection)` lowers to **17 flattened params** (`capability-id` 2 + `input` 2 + `option<connection-info>` 13 — connection-info = connection-id/integration-id/parameters strings + two `option<string>`). Wasm declared locals begin *after* the params, but the emitter's ~100 `DIRECT_*_LOCAL` scratch slots are **absolute indices with fixed i32/i64 types**, and a pure workflow with Split/While already touches ~40 of them across a wide range. The lifecycle export works only because it has 2 params (`input` folds onto locals 0/1). Going from 2→17 params shifts the declared base by 15 and invalidates every hardcoded constant index/type. So a direct capabilities export requires either (i) a per-ABI declared-local layout that re-lands all ~100 constants at their absolute indices/types given P=17 (fragile, high-risk), (ii) a **thin adapter/shim component** that exports `capabilities.invoke` and forwards to the workflow's `lifecycle.invoke` (no `DIRECT_*` constants — simple marshaling — but adds a component + wac wiring; the design pre-emptively dismissed this before the obstacle was known), or (iii) **host-side dispatch** (the host invokes the child's `lifecycle.invoke` when an agent step targets a workflow-agent id; sidesteps the signature match entirely). This is a genuine architecture fork; slice d stands regardless.

**Status — slice d LANDED (2026-07-15):** a PURE, non-durable, invoke-ABI workflow now compiles with **zero `runtara:workflow-runtime/runtime` imports** and executes with **no runtime host attached** (its terminal result is solely the in-band invoke return value). Opt-in via `RUNTARA_DIRECT_OMIT_RUNTIME=1` (compile-time); the `WorkflowFeatureSummary::needs_runtime` guard keeps it SOUND — any workflow that would emit a `runtime.*` call keeps the import (a misclassification fails loudly at component validation via a poisoned import index, never a silent miscompile). `DirectCompilationResult::omit_runtime` carries the effective decision so re-emit paths stay consistent. This is the composition-safe, agent-shaped artifact the remaining slices build on. **Next: slice a** (`WorkflowAbi::AgentCapabilities` export + a compile-time non-suspending gate). Caveat: a *top-level* run of an omit-runtime workflow does not yet persist status/output (no `runtime.complete` fires; the environment does not yet record from `InvokeExit`) — the composition path (slices b/c) invokes the child via the parent, so this is a deferred concern, not a blocker.

I confirmed the load-bearing claims against source: `WorkflowAbi` has exactly two variants and `emit_world_wit` toggles only the export line (`component.rs:64,213,227-232`); the lifecycle contract is `invoke(list<u8>) -> result<outcome,error-info>` with `outcome = completed(list<u8>) | suspended(list<wake>)` (`runtara-workflow-lifecycle.wit:39-44`); the agent contract is `invoke(capability-id, input, connection) -> result<list<u8>,error-info>` with no suspend arm (`runtara-agent.wit:38-42`); `HostImport` already omits runtime from the wac and bubbles it via the trailing `...` (`component.rs:244-267`); and a golden-snapshot tripwire guards the invoke world (`component.rs:316`). The maps are accurate. Design + build plan follows.

---

# Workflow-as-Agent: Design + Build Plan

## 1. The exact gaps that block workflow-as-agent

A compiled workflow exports `runtara:workflow-lifecycle/lifecycle@0.1.0`; a parent that runs an Agent step imports `runtara:agent-<id>/capabilities@0.3.0` and hand-lowers a call to it. Composition validates structurally (`EncodeOptions{validate:true}`, `compile.rs:598`), so the two must line up exactly. They diverge on four type-level axes plus three system-level ones:

**Type-level (WIT) gaps**
- **Package**: `runtara:workflow-lifecycle` vs per-agent `runtara:agent-<id>@0.3.0`.
- **Interface name**: `lifecycle` vs `capabilities`.
- **Arity**: `invoke(input)` [1] vs `invoke(capability-id, input, connection)` [3].
- **Success arm**: `outcome{completed|suspended}` vs bare `list<u8>` — **no suspension channel** in the agent shape. This is the one semantic (not cosmetic) gap.
- Non-gap: `error-info` is byte-for-byte identical across `runtara:abi/types` and the per-agent copy; the component model matches records structurally. The error arm interoperates for free.

**System-level gaps**
- **Runtime-import asymmetry**: a workflow imports `runtara:workflow-runtime/runtime@0.1.0` (`component.rs:220`) and calls `runtime.complete`/`fail`/`checkpoint`/`durable-sleep`; a plain agent imports **no** runtime interface. Composed into a parent, a child's runtime calls resolve — via the shared `...` bubble — to the **parent's** host runtime and **corrupt the parent instance** (a child `runtime.complete` would terminate the parent). This is the hidden landmine that makes naive composition unsafe.
- **Staging/registration gap**: nothing stages a `workflow.wasm` as `runtara_agent_<id>.wasm` (+ sidecar `meta.json`) in `components_dir`. `agent_component()` (`component.rs:200`) and `resolve_agent_component_dependencies` (`artifact_metadata.rs:252`) resolve agents purely by that filename convention.
- **Catalog gap**: the catalog is built once at boot from a hardcoded native-crate list (`bundle-emit/main.rs:22`) and a `runtara_agent_*.wasm` + `.meta.json` dir scan (`dispatcher.rs:122`), then snapshotted immutable (`server.rs:951`). `meta.json` is a hard requirement (`dispatcher.rs:155`). A workflow is invisible to validators, the step-picker, closure checks (E124), and entitlements without a synthesized `AgentInfo`.

EmbedWorkflow is **not** a vehicle here: it inlines the child's `ExecutionGraph` source into the parent at compile time (`plan.rs:692-731`, `embed_workflow.rs:468`) and shares the parent's instance/checkpoints. It cannot invoke a compiled `.wasm`.

---

## 2. Target architecture: **component composition** (recommended), host-dispatch deferred

Two candidates for *invoking* a workflow as an agent:

**A — Component composition.** The child compiles to a component that exports `runtara:agent-<id>/capabilities@0.3.0`, is staged as `runtara_agent_<id>.wasm`, and is picked up by the **existing** `emit_wac` (`component.rs:237`) / `emit_agent_invoke` (`agent_invoke.rs:30`) / `emit_agent_plan` (`agent.rs:192`) path with **zero parent-side change**. Child code is embedded in the parent artifact.

**B — Host-side dispatch.** Register the workflow in the catalog; at runtime a new guest→host sub-invoke instantiates the child separately and calls `execute_invoke` (`workflow.rs:480`), giving the child its own instance_id/checkpoints and letting `outcome::suspended` cross the boundary.

**Recommendation: composition (A) for the buildable slices.** Rationale, weighed against the maps:

- **A reuses the entire agent substrate unchanged.** `emit_agent_invoke`, `emit_agent_plan` (input-mapping, validation, retry/backoff, durable checkpoint, onError routing) and the whole import-matching path (`core_imports.rs:732-737` keyed on the `runtara:agent-` prefix) already do "call a component's `invoke`." A capabilities-exporting workflow drops in with no changes to `agent.rs`/`agent_invoke.rs`.
- **B's only advantages are exactly what v1 excludes.** B buys separate durable identity and suspend propagation. But v1 workflow-as-agent is **non-suspending, non-durable** (see §5), so B's greenfield cost — a new guest import, a new host path distinct from both the test-only `dispatcher.rs` and top-level-only `execute_invoke`, and the hard problem of absorbing a child's `suspended(list<wake>)` into the parent's own suspend set with correct durable replay — buys nothing.
- **The composition landmine is neutralized by the first slice, not by B.** A child compiled with **zero runtime imports** (slice d) makes no `runtime.*` calls and returns its result purely as the invoke value, so it cannot corrupt the parent instance. Slice d is what makes A *safe*, and it's independently valuable.
- **Fan-out constraint is not triggered.** The ruled-out pattern is host-orchestrated *fan-out* (`feedback_no_host_orchestrated_fanout`). Composition is in-guest by construction. A future B is a *sequential* nested invoke, which is explicitly *not* fan-out — so B stays open as the eventual path for suspending children, it's just not needed now.

**Decision: build composition. Name host-dispatch as the deferred architecture for the day a workflow-as-agent must be durable or suspend** (it requires widening the shared boundary to carry `outcome` in `runtara:abi` and having the parent linearize the child's wakes — out of scope for v1).

---

## 3. First slice — slice d: `durable:false` workflow omits the runtime import

**Goal.** A non-durable, non-tracing, invoke-ABI workflow compiles with **no `runtara:workflow-runtime/runtime` import and zero runtime calls**, returning its terminal result solely as the `invoke` return value. This is the smallest correct increment and the foundation both for safe composition (§2) and for the capabilities export (slice a). It ships value on its own (smaller artifact, no shared-runtime side effects) and is independently verifiable.

Scope note: this omits **runtime** only. stdlib stays imported/called (`init-manifest`/`build-source`/`apply-mapping`, `core_module.rs:487+`); true agent byte-identity (also dropping stdlib) is explicitly *not* this slice.

**Files / symbols that change**
- `workflow_features.rs` (near `:121`): add a precise `needs_runtime()` predicate. Compute `omit_runtime = abi==InvokeHostImports && !root_durable && !track_events && !requires_composed_imports()`. Thread it from `compile.rs:796/968` where `feature_summary` is already in hand.
- `component.rs:213 emit_world_wit`: add a `runtime: bool` param; gate the `import runtara:workflow-runtime/runtime` line (`:219-220`) on it. Mirror in the bytes-authoritative resolve builder `compile.rs:1009 build_direct_component_resolve_configured` (gate the `RUNTIME_WIT` push and the hardcoded import at `:1018/1055`). **Both sites must move together** (golden-snapshot tripwire at `component.rs:316`).
- `core_imports.rs:148 require_all`: add `runtime_imported: bool`; when false, do **not** require the 19 runtime funcs (`require_import`, `:702`). Assert (debug) that no lowerer emitted a runtime call under omit.
- `core_module.rs:543`: suppress the additive `runtime.complete` when `omit_runtime` — the invoke success path `emit_invoke_ok_completed_return` (`:569`) is already authoritative.
- `compile.rs:1116 emit_runtime_fail_return` + `abi.rs:527/537`: suppress `runtime.fail` under omit; confirm the error path returns `Err(error-info)` through the lifecycle result rather than only via `runtime.fail`. **If it doesn't today, this slice must add host-side terminal-status recording from `InvokeExit::Completed`/`Failed` in `execute_invoke` (`workflow.rs:480`).**

**The one real risk (verify first).** Today `runtime.complete`/`fail` double as host-side status recording. The map's open question: does `execute_invoke` record status purely from the returned `outcome` when complete/fail never fire? This is the load-bearing e2e assertion. Host/wac side is otherwise safe — `HostImport` already bubbles an unsatisfied runtime import and `add_runtime_to_linker` is non-invasive (`workflow.rs:249`, `runtime_host.rs:196-201`); omitting the import entirely just leaves nothing to bubble.

**Tests**
- *Unit*: `emit_world_wit(runtime=false)` contains no runtime import; `require_all` tolerates missing runtime; **new golden snapshot** for the runtime-omitted invoke world (sibling to `component.rs:316`). Compile a trivial `durable:false` passthrough (single Finish, no agents) and assert the emitted world/core import section has zero `runtara:workflow-runtime` funcs and no `runtime.complete`/`fail` call sites.
- *e2e* (via the `e2e-verify` skill, per `feedback_always_e2e_verify`): create the passthrough workflow, compile through the server HTTP API, run it, and assert (a) it returns the correct output, **and (b) the instance's terminal status is recorded correctly despite `runtime.complete` never firing**. This is the direct test of the open question.
- *A/B parity*: same workflow, flag-off (runtime imported) vs flag-on (omitted), must produce byte-identical execution output — reuse the existing 48/48 invoke A/B parity harness (`project_embedded_workflow_runner`).

**Gating so nothing regresses**
- Env kill-switch `RUNTARA_DIRECT_OMIT_RUNTIME` (auto-derive by default; force-off = rollback), consistent with `runtime_binding_from_env`/`store_freeing_sleep_from_env` (`compile.rs:635/743`).
- Active **only** under `InvokeHostImports` + non-durable + non-track_events. `CliRunHttp` (needs `load-input`/`complete`) and any durable or debug/tracing build stay on the current path untouched.
- Byte-instability guard (open question in the map): the omit decision is recorded in the manifest sidecar so a given artifact's shape is explicit; debug builds keep runtime (track_events uses it), and debug≠shipped-artifact parity is explicitly not required.

---

## 4. Slice sequence after the first

**Slice a — capability/signature adapter (new `WorkflowAbi::AgentCapabilities`).** Depends on d.
- `component.rs:64`: add the variant. `emit_world_wit` (`:227-232`) gains a third arm exporting `runtara:agent-<id>/capabilities@0.3.0`; the emitter emits a 3-arg `invoke` that **ignores `capability-id`** (or validates it equals the single synthetic capability), **ignores `connection`** for v1, runs the same core body as lifecycle, and **unwraps `outcome`→`list<u8>`** on completed. Because d already stripped runtime and made the result the return value, this is a small delta over the lifecycle emit.
- **What `suspended` means to a capability caller (v1):** it *cannot occur* — the workflow is compile-time-validated non-suspending (§5), and the capabilities `list<u8>` arm has no suspend channel. Defensively, if reached, return `Err(error-info)` with `category=permanent, code=E-workflow-suspended-in-agent-context`. Prefer the new ABI variant over a separate shim component (the emitter already toggles the export by ABI in one match; a shim adds a crate, wac wiring, and a second runtime boundary).
- *Verify* via `dispatcher.rs:243-351 test_capability` — the existing host-mediated typed `invoke` is the perfect standalone harness for the compiled workflow-agent before any composition exists.
- Effort **M** / risk **M–H** (the exported package/interface/version must match `runtara:agent-<id>/capabilities@0.3.0` exactly or slice b's `validate:true` composition hard-fails).

**Slice b — composition / staging machinery.** Depends on a.
- Stage the child (compiled with the AgentCapabilities ABI) as `runtara_agent_<id>.wasm` in `components_dir`; ensure `resolve_agent_component_dependencies` (`artifact_metadata.rs:252`) finds it and the package/version/filename triple from `agent_component()` (`component.rs:200-211`) line up. Parent-side `emit_wac`/`emit_agent_invoke`/`emit_agent_plan` need **no change** — a Step::Agent whose `agent_id` equals the child id is auto-imported and composed. The child's transitive agents are already in `components_dir`; the child has **no** runtime import to satisfy (slice d), which is exactly why composition is clean.
- Effort **S–M** / risk **M** (`validate:true` fails loudly on any mismatch — a feature, not a hazard).

**Slice c — catalog entry.** Depends on a; parallelizable with b.
- Synthesize `runtara_agent_<id>.meta.json` = `AgentInfo` (`agent_meta.rs:254`) with one `CapabilityInfo` (`:308`) whose `inputs` = the workflow input schema, `output` = the workflow output schema, `id` = a fixed `"invoke"`. Stage beside the `.wasm` so the boot-time `from_dir` scan registers it (`dispatcher.rs:122`); this makes v1 registration = "drop wasm+meta in `components_dir`, restart." This lights up the step-picker, `validate_graph`, E124 closure, and entitlements (`config.rs:338`).
- Do **not** route through the hardcoded `emit-meta` crate list (`bundle-emit/main.rs:22`); synthesize from the workflow's own I/O schema. Boot-immutable catalog + runtime registration (overlay/mutable catalog) is a follow-up.
- Effort **M** / risk **M**. Watch id collisions with native agents (`canonical_agent_id` kebab keying, `agent_meta.rs:1825`; `dispatcher.rs:171` asserts filename↔meta.id agreement).

**Slice e — connection plumbing.** Depends on b/c. **Deferred; v1 ignores `connection`.**
- v1: the child ignores the `connection` arg — a workflow gets connections through its own steps' `connection_id`s, resolved by the proxy on outbound `wasi:http` (secrets never enter the guest; `emit_agent_connection_args` writes id-only, `agent_invoke.rs:65-104`). Real threading (which child step receives a passed-in connection-id?) is a later semantic decision.
- Effort v1 **S** / risk **L**; real threading later **M–H**.

**Dependency graph:** d → a → {b, c} → e. Ship d and a independently (a verifiable via `test_capability` with no composition). b+c together deliver the first end-to-end "parent workflow calls a compiled child workflow as an agent."

---

## 5. Hardest correctness question + safe initial policy

**Question: suspension semantics of a nested invoke.** The agent success arm is bare `list<u8>`; a workflow can return `outcome::suspended(list<wake>)` (Delay, WaitForSignal, durable sleep, drain). Composed as a capability there is no channel to carry a suspend, and today suspension is handled entirely out-of-band by the host scheduler (`park_invoke_suspend`, `embedded.rs:239-273`, then re-invoke) — there is no mechanism to move a child's wakes into a parent's suspend set across a component boundary.

**Safe v1 policy: workflow-as-agent must be non-suspending, enforced at compile time — not merely at runtime.**
- A workflow compiled with the AgentCapabilities ABI must be `durable:false` **and** contain no suspend-capable feature. Add a fail-loud validation rule (new error code, e.g. `E-workflow-not-agent-eligible`) driven off `feature_summary`: reject Delay, WaitForSignal, durable sleep, and any durable step. This turns the type-level impossibility (no suspend arm) into a clear authoring error instead of a runtime trap.
- This dovetails with slice d: non-durable ⇒ no runtime ⇒ no durable sleep ⇒ structurally cannot suspend. The gate and the artifact shape reinforce each other.
- The eventual escape hatch (durable/suspending children) is **host-side dispatch (B)** with a widened `outcome`-carrying sub-invoke in `runtara:abi` and a parent that absorbs the child's wakes — deliberately out of v1, and unblocked by the fan-out prohibition because a single nested invoke is sequential, not fan-out.

**One-line build order:** slice d (omit runtime, verify status-from-return) → slice a (capabilities ABI + non-suspending gate) → slice b + c (stage/compose + synthesized meta.json) → slice e deferred.
