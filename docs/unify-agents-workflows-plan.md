# Unify Agents & Workflows: Host-Imports + Invoke Export

**Status:** Draft for review (produced via multi-agent research + adversarial verification)
**Date:** 2026-07-14
**Goal:** Replace the workflow guest's HTTP loopback to core (input / lifecycle / events / checkpoints / signals) with **host-function imports** satisfied natively in-process, and change the workflow's top-level export from `wasi:cli/run` to an **`invoke(input) → result<outcome, error-info>`** function shaped like an agent — so agents and workflows become the same kind of component.

---

## 0. TL;DR / verdict

The change splits into a **safe half** and a **risky half**, and they should ship separately:

- **Safe half (Phases 1–2): eliminate the HTTP loopback.** Make the `runtime` interface a host import (satisfied via `add_to_linker`, delegating to `runtara-core::instance_handlers` over `Arc<dyn Persistence>`), while keeping the `wasi:cli/run` export and blocking durable-sleep. This alone achieves *"workflows contact core via host functions, not HTTP"* with a low-risk, differentially-verifiable diff. **Recommended to land first and independently.**
- **Risky half (Phases 3–6): the `invoke` export + suspension-as-return-value.** This is where the real unification (agent == degenerate workflow) lands — and where verification found **two confirmed correctness blockers** and a test-harness blind spot. It must be gated behind prerequisites (below), not shipped with the safe half.

The core module **already imports and calls** the full `runtime.*` surface by numeric index (`core_imports.rs:730–735`, `core_module.rs:445/493`) — so for the safe half, *only the binding source changes*, not the module. That's the single biggest de-risking fact.

---

## 1. Target architecture: one kind of component

There is ONE executable component — an **"invoke component."** It exports an invoke-style function that takes input as a call argument (`list<u8>` JSON bytes) and returns its result as the return value, and it **optionally imports a `runtime` host interface** for durability/lifecycle.

- **Agent** = the degenerate case: exports `invoke`, imports **no** `runtime` interface, its success arm is a terminal payload, it never suspends. (Already true today: `runtara-agent.wit:38`.)
- **Workflow** = the general case: exports `invoke`, imports `runtime` (host-satisfied), and its success arm can be `suspended`.

The two host execution paths — the workflow runner (`crates/runtara-component-host/src/workflow.rs`) and the agent dispatcher (`dispatcher.rs`) — collapse into one `ComponentExecutor` (see §4). The external control plane is untouched: the guest simply stops being a client of core's guest-protocol HTTP router; the *management* router that external callers use is a separate axum router over the same `Arc<dyn Persistence>` (§7).

---

## 2. The unified contract (WIT)

Agents and workflows share the **error type and the bytes-in/result-out convention**, but not the exact `invoke` signature — agents multiplex capabilities and carry a per-call connection; workflows have a single entry, no top-level connection, and a wider success arm for suspension.

```wit
// runtara:abi@0.1.0 — neutral shared vocabulary.
// INTERIM: reuse runtara:agent/types@0.3.0.{error-info, connection-info}; mint runtara:abi later.
interface types {
  record error-info { code: string, message: string, category: string, severity: string,
                      retryable: bool, retry-after-ms: option<u64>, attributes: option<string> }
  record connection-info { /* shared with agents */ }
}

// Agent world: UNCHANGED except the `use` target moves to runtara:abi.
world agent { export capabilities; }        // capabilities.invoke(cap-id, input, connection) -> result<list<u8>, error-info>

// Workflow world: NEW export shape (replaces `export wasi:cli/run@0.2.3`).
interface lifecycle {
  use runtara:abi/types.{error-info};
  record signal-wait { checkpoint-id: string, deadline-ms: option<u64> }
  variant wake { at(u64), on-signal(signal-wait), on-resume }        // why it stopped early / how to re-invoke
  variant outcome { completed(list<u8>), suspended(wake) }           // completed == the agent's ok-arm
  invoke: func(input: list<u8>) -> result<outcome, error-info>;
}
world workflow {
  import runtara:workflow-stdlib/json@0.1.0;
  import runtara:workflow-runtime/runtime@0.1.0;   // now HOST-satisfied via add_to_linker, not composed
  // import runtara:agent-<id>/capabilities@0.3.0; (one per agent used)
  export lifecycle;                                // was: export wasi:cli/run@0.2.3
}
```

**The `runtime` import interface: 3 funcs removed, 16 kept** (`runtara-workflow-runtime.wit:9–89`):
- **Removed → moved to the invoke boundary:** `load-input` → the `input` argument; `complete` → `Ok(outcome::completed(bytes))`; `fail` → `Err(error-info)`.
- **Kept → host `add_to_linker` surface, delegating to persistence:** `instance-id, custom-event, debug-mode-enabled, breakpoint-pause, heartbeat, is-cancelled, check-signals, poll-custom-signal, now-ms, durable-sleep, blocking-sleep, get-checkpoint, checkpoint, handle-checkpoint-signal, record-retry-attempt, durable-sleep-checkpoint`.

`error-info` (reused verbatim) is strictly richer than today's bare `string` in `runtime.complete/fail`.

---

## 3. Suspend / resume design (the crux) — corrected by verification

Today, **suspension is not a guest return value — it's out-of-band DB state** (`status='suspended'/'sleeping'` + `sleep_until` + checkpoint), and resume is **re-invoke + replay-from-start** (never stack preservation). Durable steps consult their checkpoint as a result cache to skip completed work (`RUNTARA_CHECKPOINT_ID` is passed but never consumed). Notably, **durable sleep does not exit today — it *blocks*** in-process, pinning a whole `Store` + linear memory resident for the sleep duration.

**Decision: workflow `invoke` is a strict superset of agent `invoke`** — the success arm widens from `list<u8>` to `variant outcome { completed(list<u8>), suspended(wake) }`. Putting suspension in the **return type** makes the host↔guest contract type-checked at the component boundary and removes today's coupling where `WorkflowExit::Completed` can actually mean "suspended, go re-read the DB."

**Two-stage rollout:**
- **Stage 1 (option-b, blocking):** `runtime` becomes a host import but durable-sleep/wait stay resident — the host closure async-sleeps in-process. Export stays `wasi:cli/run`. **Zero emitter change.** Behavior identical to today minus the HTTP transport.
- **Stage 2 (option-a, re-invoke):** flip the export to `invoke → outcome`; suspension points emit `Return suspended(wake)`; the runner tears down the `Store` and hands the wake reason to the scheduler. This frees the `Store` during any wait — strictly better than today's resident poll loop.

### ⚠️ Two blockers that reshape Stage 2 (both CONFIRMED against code)

**Blocker A — Wait-for-signal replay deadlock.** Custom signals are **destructively `DELETE`d on poll** (`signals.rs:70–117`), and the wait step is **never result-checkpointed** (`grep emit_checkpoint_* wait.rs` = 0). Under the blocking model this is masked because the guest stays resident. Under option-a, any `[wait → later-suspension]` workflow re-reaches the wait on replay, re-polls the already-deleted signal, and **re-suspends forever**. *Prerequisite before converting the wait poll loop:* make the received signal durable — either checkpoint the wait keyed by its deterministic signal id (mirror `agent.rs:175/317`), or make `take_pending_custom_signal` mark-delivered-idempotently (re-readable by `checkpoint_id`) instead of `DELETE`.

**Blocker B — Retry-backoff side-effect amplification.** `runtime_durable_sleep_checkpoint` is **one shared import driven by four callers** — `delay.rs:116`, `agent_retry.rs:140`, `split_retry.rs:194`, `embed_retry.rs:198`. The host can't tell a Delay from a retry backoff. Retry loops reset the attempt counter each invoke (`agent.rs:194`) and checkpoint **only on success** (`agent.rs:316`). Converting that import to a suspend point makes every backoff suspend+replay and **re-invoke every prior failed attempt → O(n²) re-execution of non-idempotent external calls**. *Prerequisite:* do **not** route retry-backoff sleeps through the suspend path — give retry a distinct always-blocking host import (keep option-b for retries), or checkpoint the agent invoke **per-attempt** (not only on success).

**Also required for Stage 2:**
- **Wait absolute deadline must be persisted** (MAJOR/CONFIRMED). The wait deadline is a recomputed local (`wait.rs:280`, `now_ms()+timeout_ms`), never persisted. Under re-invoke it re-computes `now+timeout` each resume and **can never time out**. Persist the absolute deadline (like `sleep_until`) at first suspend; the re-invoked guest reads it.
- **On-signal wake path is net-new** (MAJOR). The wake scheduler only relaunches instances whose `sleep_until` is due (`wake_scheduler.rs:126`). A torn-down `on-signal` instance is never woken by a custom-signal insert. On custom-signal insert for a suspended instance, set `sleep_until=now` (reuse the wake scheduler) or add an explicit waker.
- **Durable-sleep Stage-2 is a tri-state, not a one-line `Return`** (MAJOR/CONFIRMED). The host closure must be **non-blocking** and return `{continue | suspend(deadline)}`; the emitter needs a new decision point at the delay/retry/durable-sleep site (`delay.rs:113` currently calls sleep then *unconditionally* continues). The "port resume-remaining sleep math into the closure" wording contradicts "free the Store" — drop the in-closure sleep for option-a.

### The replay contract (preserved verbatim)
Re-invoke with the same `instance_id` + the same persisted input; durable steps hit their checkpoint cache (agent per-step; Split whole-result — a mid-iteration Split crash still re-runs items 0..n, an *existing* property). `durable:false` steps re-execute (emitter already elides their checkpoint/sleep emission). **Critical input seam:** because input becomes a call argument, the host **must re-fetch the persisted, enriched input on wake and pass it** — `wake_scheduler.rs:217` currently launches with `input:{}` (harmless only while `load-input` reads the record). It must pass `persistence.get_instance().input` (the enriched bytes, not `LaunchOptions.input`) on both first-run and wake, or every `data.*` reference silently resolves to null (the known envelope hazard).

---

## 4. Unified executor

One `ComponentExecutor` with a single `Store`-data type: `{ wasi, http, table, hooks }` shared, plus three optionals that make *"agent = workflow minus the runtime import minus the interruption rings"* true in code:
- `Option<RuntimeHostCtx>` (workflow-only: `Arc<dyn RuntimeHost>` + `instance_id` + `tenant_id`),
- `Option<WorkflowLimiter>` and `Option<Termination>` (interruptible-only).

Agents pass `None` for all three. The executor adds the `runtime` interface via `add_to_linker_async` (closures read `&mut state.runtime`); the load/execute path keeps the **dynamic `get_export_index` + `TypedFunc`** pattern from `dispatcher.rs:240–262` and drops the hard `CommandPre`/`wasi_cli_run` requirement at `workflow.rs:273`. Interruption rings (epoch callback + watchdog + limiter, `workflow.rs:341–382`) become opt-in `RunControls`, default OFF for agents (matching `registry.rs` epoch `u64::MAX`), ON for workflows.

**`RuntimeHost` is a hand-written async trait DEFINED in `component-host`** (keeping its deps to agent-wit/dsl/wasmtime only — it has *no* persistence dep, by design) **and IMPLEMENTED in `runtara-environment`**, delegating to the async `instance_handlers` (`handle_checkpoint/handle_sleep/handle_poll_signals/...` over `InstanceHandlerState = Arc<dyn Persistence>`). Do **not** delegate to `EmbeddedBackend` — it wraps every call in its own current-thread `block_on` (`embedded.rs:33–49`) → nested-runtime panic inside an async closure, and it has silent semantic gaps (`poll_signals` returns `(None,None)` at `embedded.rs:426`; `checkpoint()` lacks the status-guard the HTTP handler enforces at `checkpoint.rs:40`). Persistence is already on hand at the construction site: `EmbeddedWasmRunner` holds `persistence: Arc<dyn Persistence>` (`runner/embedded.rs:54`) and builds the executor there; move `instance_id/tenant_id` from env vars into typed `WorkflowRunSpec` store state.

---

## 5. Phased plan

### Phase 0 — De-risking spikes (run BEFORE committing dependent phases)
Three cheap, high-information spikes; each is the acceptance gate of a later phase pulled forward:
1. **Composition spike (B, ~1h — highest value/lowest cost).** Drop the `let workflow-runtime = new …` line + the `…workflow-runtime` spread from `emit_wac` (leave the trailing `…`); run `compose_workflow_component_in_process`; assert encode+`validate:true` succeeds AND `Component::component_type().imports()` lists `runtara:workflow-runtime/runtime`. Proven for *transitive* WASI imports; **unproven for a directly-declared workflow-logic import** — this settles it.
2. **Host-binding spike (C = Phase 1 acceptance).** Hand-write a minimal invoke test component that imports `runtime`; bind all 19 funcs via `add_to_linker_async` against a real `Persistence`; drive checkpoint round-trip + poll-signal-after-external-insert + durable-sleep-sets-`sleep_until`. **No in-repo precedent exists** for binding a custom (non-WASI) host interface with records/variants/options — prove the marshaling (bindgen from the retained `RUNTIME_WIT` is the low-risk route).
3. **Export-return spike (E).** Emit an `invoke` export returning a fixed `completed(bytes)`; read it back via the dynamic `TypedFunc<(Vec<u8>,),(Result<Outcome,ErrorInfo>,)>`. The emitter has **never written a `result<list<u8>, record>` into a canonical return area** (today's `run` returns an empty scalar tag) — the outcome-variant lowering is genuinely new (not "verbatim reuse of `DIRECT_AGENT_RESULT_*`", which are the *read* side of imported agent calls).

### Phase 1 — Host runtime interface (dormant) · risk: medium
Prove host-import lifecycle works while production still uses `wasi:cli/run` + the composed runtime component. Define the `RuntimeHost` trait in component-host; implement in runtara-environment over `instance_handlers`; add an `add_to_linker_async` binding in a **new, non-default** linker variant. **Acceptance:** unit test drives all 19 `runtime.*` host funcs end-to-end against `Persistence`; existing suite green; production path byte-identical.

### Phase 2 — Switch composed runtime → host import (keep `wasi:cli/run`, blocking sleep) · risk: high
The safe-half payoff. `emit_wac`: drop the runtime `new`+spread (keep trailing `…` so runtime surfaces as a component import like WASI). Remove `workflow-runtime` from `DIRECT_SHARED_COMPONENT_REQUIREMENTS`. Wire the Phase-1 linker into the production execute path; thread `instance_id/tenant_id` via typed store state. Host implements **all 19** funcs incl. `load-input/complete/fail` (export is still `wasi:cli/run` this phase). **Acceptance:** compiled component now *lists* a `runtime` import; full 106-fixture A/B corpus green with the composed runtime removed; durable-sleep-across-simulated-restart, wait-for-signal delivery, retry-backoff resume all pass (blocking).

> After Phase 2 the HTTP loopback is gone. This is the shippable milestone for *"workflows use host functions, not HTTP."*

### Phase 3 — Invoke-export WIT + emitter variant (behind `WorkflowAbi` flag, default off) · risk: high
New workflow world v0.2.0: `export lifecycle.invoke(input)->result<outcome,error-info>`; keep `import runtime`. Update **both** emitters (`component.rs:120` `emit_world_wit` AND `compile.rs:836` `build_direct_component_resolve_with_agents`) in lockstep. Emitter `WorkflowAbi::{CliRunHttp default, InvokeHostImports}`. Under the new variant: (a) **fold the two invoke params onto locals 0/1** by shrinking the first declared-local group by two i32, so `DATA_PTR/DATA_LEN` land on param0/param1 and the ~100 `DIRECT_*_LOCAL` constants are unchanged; (b) read the input arg instead of `runtime.load-input`; (c) **write the outcome result variant** (fresh return-area offsets — the ok arm is a variant, *not* the agent read-offsets; err arm may coincide) via a single `emit_invoke_return_err` helper; (d) delete `load-input/complete/fail` from `require_all` + the index struct + call sites. Define a canonical `string → error-info` wrapping for non-structured failures (template/mapping/Filter errors reach `fail` as plain strings today, `compile.rs:879`). **Acceptance:** golden invoke-WIT snapshot; all 106 fixtures **compile** under the flag; a `compile::tests` assertion that the invoke body's param+local count is unchanged; **plus a focused output-ownership test** (the param-count assertion does NOT guard the return-area writer). Default path untouched.

### Phase 4 — Differential parity harness + suspend wiring · risk: high (behavior-changing)
Parametrize `direct_wasm_execute` over the ABI; add a capturing `add_to_linker` shim recording the same `{output,error,events,sleeps,checkpoints}` tuple the mock HTTP server records today. Generalize the early-return-to-suspend pattern (`checkpoint.rs:70`) to durable-sleep-checkpoint (as a **tri-state**, per §3) and the wait poll loop (`Return suspended(wake)`). Add `WorkflowExit::Suspended`; route `at` → wake scheduler, `on-signal` → **new host signal-wait table + custom-signal-insert waker** (the missing wake path). Re-fetch persisted enriched input on wake.

> **Gate Stage-2 (this phase) on the Blocker prerequisites (§3):** durable wait-result, retry backoffs kept off the suspend path, persisted wait deadline, on-signal waker. **And expand the corpus** — today every wait fixture is `wait→finish` (`fixtures/wait_for_signal.json`), so both ABIs pass while the deadlock/amplification hide behind a downstream suspension the corpus never constructs. Add `[wait → durable delay → finish]`, `[wait → wait]`, `[retryable-agent-with-durable-backoff → durable delay → finish]`, `[split of N items each with a durable per-item delay]` — each run through a simulated suspend/resume.

**Acceptance:** 106/106 differential parity old-vs-new on `{output,error,events,sleeps,checkpoints}` **plus** the new post-suspension fixtures; durable sleep across a real simulated restart resumes with *remaining* time; externally-delivered wait-for-signal is consumed by a host-import guest; split mid-iteration crash replays correctly; retry backoff does not re-invoke already-attempted attempts.

### Phase 5 — Flip default + CI drift gates · risk: medium
Flip `WorkflowAbi` default to `InvokeHostImports`. Add a workflow-invoke drift detector (modeled on the agent dispatcher drift test) + golden invoke-WIT snapshot. Extend the `core_imports` `require_all` fail-loud guard to cover the invoke export. Extend `wit-package` CI to resolve the invoke-export + runtime host-import worlds; keep the `components-build` direct-wasm smoke as the real drift gate. **Acceptance:** CI green with default flipped; drift detector fails on a deliberately mismatched invoke signature; `bindings.rs` regen scoped to the interface change.

### Phase 6 — Retire composed runtime crate + workflow HTTP backend · risk: high (irreversible)
**Gated on N green-parity releases; must NOT co-land with the Phase-5 flip.** Delete the `runtara-workflow-runtime` guest crate (but **keep `RUNTIME_WIT`** in `runtara-workflow-wit` as the canonical host-interface contract — makes an out-of-process runner reversible). Remove the workflow HTTP SDK backend usage + `execute_via_cli` A/B axis. Gate the core guest-protocol HTTP router off for embedded runs (dead weight once guests use host imports) — after a final sweep confirms no surviving non-guest consumer. **Acceptance:** corpus green for N releases; final grep confirms no `runtara-server`/`management-sdk`/`runtara-ctl` caller targets the guest-protocol port; agent WIT + dispatcher/drift tests untouched.

---

## 6. What stays HTTP (external control plane — untouched)
The entire `runtara-environment` **management** router remains a service API for external callers: `POST /instances` (start), stop, resume, `GET /instances/{id}` (status), list, `POST signals` + `signals/custom` (external submit), `GET checkpoints`, events, steps, scope ancestors (`http_server.rs:2036`). External callers that keep using it: the `runtara-server` `ExecutionEngine` start (`execution_engine.rs:933`); human/MCP signal + action responses (`workflow_runtime.rs:293`); status/list/stop/resume polling incl. drain/resume; dashboard/console + MCP reads; the reports `workflow_runtime` provider. **The management router and the guest-protocol router were always separate axum routers over one shared `Arc<dyn Persistence>`** — only the guest stops being a client, so no handler refactor is needed.

---

## 7. Risks (merged: synthesis + verification)

| Sev | Risk | Mitigation |
|---|---|---|
| **Blocker** | Wait-for-signal replay deadlock under option-a (signals `DELETE`d on poll, wait not checkpointed) | Make received signal durable (checkpoint by signal id, or idempotent mark-delivered) **before** converting the wait loop |
| **Blocker** | Retry-backoff O(n²) side-effect amplification (shared durable-sleep import, success-only checkpoint) | Keep retry backoffs on the blocking path (distinct import) OR checkpoint per-attempt; never route them through suspend |
| High | Local-index rebase: invoke `input` occupies locals 0/1, shifting ~100 `DIRECT_*_LOCAL` indices | Fold params onto 0/1 (shrink first local group by 2×i32); assert param+local count unchanged |
| High | Return-area writer for `result<outcome,error-info>` is net-new (not verbatim agent-offset reuse) | Isolated spike (Phase 0.3) + golden output-ownership test before the 106-fixture corpus |
| High | Wait absolute deadline resets on every re-invoke → never times out | Persist absolute deadline at first suspend; re-invoked guest reads it |
| High | Parity harness blind spot: all wait fixtures are `wait→finish` → both ABIs pass while blockers hide | Add post-suspension fixtures (wait/retry followed by a suspension) run through resume |
| Medium | On-signal suspension has no wake path | Custom-signal insert for a suspended instance sets `sleep_until=now` / explicit waker |
| Medium | wac-graph leaving a *directly-declared* import unsatisfied is unproven in-repo | Phase 0.1 compose-and-assert-imports spike |
| Medium | Binding a custom non-WASI host interface via `add_to_linker` has no in-repo precedent | Phase 0.2 / Phase 1 acceptance spike |
| Medium | Nested-tokio panic if `EmbeddedBackend.block_on` is reused in an async closure | Delegate to async `instance_handlers` directly; `EmbeddedBackend` is reference-only |
| Medium | `EmbeddedBackend` semantic gaps (poll_signals `(None,None)`, missing status/probe guards) | Delegate to `handle_*` handlers for parity with the retained HTTP path |
| Medium | Two-emitter drift (`component.rs:120` vs `compile.rs:836`) | Change both in one commit; golden WIT snapshot; components-build smoke gate |
| Low | Stage-1 blocking sleep must persist `status='suspended'+sleep_until` before awaiting, or a watchdog-dropped long sleep orphans the instance as `running` | Persist `sleep_until()` semantics before the in-process await |
| Low | Wait-loop → on-signal removes idle heartbeats/telemetry | Audit event/telemetry consumers before Phase 4; keep a heartbeat on the wake tick if needed |
| Low | Bump-allocator leak if an invoke component is reused across runs | Keep a fresh `Store` per invoke (as both paths do today) |

**EmbedWorkflow stays inlined** (not an invoke call): a child's `DirectRunPlan` is emitted into the parent's run function sharing linear-memory locals; child failure branches via `Br(branch_depth)`, not `Return` (`embed_workflow.rs:59`). Unification holds at the **top-level artifact boundary**; the "touches every Return site" audit must include inlined-child suspend/fail returns at arbitrary `Br` nesting depth.

---

## 8. Open questions
- **Neutral `runtara:abi@0.1.0` now vs interim reuse of `runtara:agent/types@0.3.0`?** Minting up front rewrites every `crates/agents/*/wit` and forces a version bump but yields the clean end state; the plan defers it to a Phase-6 follow-up. Confirm that ordering won't calcify `agent/types` as the permanent shared home.
- **The `now-ms`/`debug-mode-enabled`/`blocking-sleep` trio:** keep inside the `runtime` host interface (simplest, small import set) or express via `wasi:clocks`/`wasi:cli/environment` for standards-alignment? (`blocking-sleep` must become a host async sleep either way.)
- **Make the `runtime` import optional per compiled workflow** — present only when the run plan actually uses durability — so a `durable:false` workflow compiles byte-for-byte shaped like an agent (export invoke, zero runtime imports)? That makes *"agent = degenerate workflow"* true at the artifact level, not just conceptually.
- **Per-iteration Split checkpoint:** under re-invoke suspension, a large Split with per-item waits re-runs items `0..suspended-iteration` each resume (O(n²) replay of cheap non-durable bodies). Is a per-iteration Split checkpoint needed to bound replay cost? Related: a durable Delay nested in a Split shares ONE sleep-checkpoint key across iterations (`delay.rs:106`, no loop index) — scope the key by loop index before enabling per-item suspension.
- **Outbound agent/LLM/object-model traffic stays on `wasi:http`** (confirmed scope: host imports cover input/lifecycle/events/checkpoints/signals only). The mock LLM proxy + object-model routes in the harness rely on this — confirm.
- **Final sweep:** does any non-guest tool (`runtara-ctl`, ops scripts) call the core guest-protocol port before it's gated off?
- **Should interruption rings (epoch/watchdog/memory limiter) apply to agent invokes once unified?** Agents run epoch `u64::MAX` today (`registry.rs:108`); per-tool-call timeouts on agents are desirable but a behavior change.

---

## 9. Escape hatch
**No standalone/out-of-process escape hatch is preserved.** The user ruled out the "workflow.wasm runs on stock wasmtime by calling our HTTP API" property (vendor protocol over a standard transport), and the out-of-process CLI process runner is **already removed** (`runner/mod.rs:22` resolves every `RUNTARA_RUNNER` value to the in-process `EmbeddedWasmRunner`). `RUNTIME_WIT` is deliberately retained in `runtara-workflow-wit` as the canonical host-interface contract, so re-adding an out-of-process runner later is one WAC `new`+spread line + one `DIRECT_SHARED_COMPONENT_REQUIREMENTS` entry + re-selecting the HTTP SDK backend — reversible even after the guest crate is retired.

---

## 10. Bottom line
The core module already imports the `runtime` surface, persistence is already on hand where the executor is built, and wac already surfaces host-satisfied imports for WASI — so **Phases 1–2 (kill the HTTP loopback, host-import the runtime) are a clean, high-value, differentially-verifiable win worth shipping on their own.** The unification's second half (invoke export + suspension-as-return) is real and achievable but carries the sharp edges: an untested return-area lowering, and — most importantly — **two confirmed correctness blockers (wait-replay deadlock, retry amplification) that require making wait-results and retry-attempts durable *before* option-a, plus post-suspension test fixtures the current corpus lacks.** Sequence accordingly: spikes → Phase 1–2 (ship) → Phase 3 spike-gated → Phase 4 blocker-gated → 5 → 6.
