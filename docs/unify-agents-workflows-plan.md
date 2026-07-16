# Unify Agents & Workflows: Host-Imports + Invoke Export

**Status:** PHASES 0–5 LANDED on main (2026-07-15): Spike B `a91e51e3` · Phase 1 `1385af8f` · Phase 2 `ea7d72c7` · lever `31c927f7` · Phase 3 `cb2c62b9` · Phase 4.1 `a5d69cb4` · Phases 4.2+5 `604a604f`. **The default compile is now the fully-unified shape** — `import runtara:workflow-runtime/runtime` (host-satisfied, no HTTP loopback) + `export runtara:workflow-lifecycle/lifecycle.invoke` (input as the argument, terminal result in-band). Live-e2e-verified on a local server: artifacts from two prior builds (composed AND host-import/cli-run) execute unchanged with no recompile; the new default compiles, executes through production `execute_invoke`, and checkpoints through the host import. Rollback levers: `RUNTARA_DIRECT_RUNTIME_BINDING=composed`, `RUNTARA_DIRECT_WORKFLOW_ABI=cli-run`. Battery green on all three axes (77/77 each).
**Follow-up slices (post-flip):** Slice 1 `29e86100` — structured `error-info` mapping (`invoke-error-fields` decomposes the JSON envelope; `context` → attributes) + per-iteration Delay sleep-key loop-index scoping (`delay-sleep-key`). Slice 3 `9c7f4645` — neutral `runtara:abi` package minting + CI drift-gate extension for the invoke WIT. Slice 2 (`e800c6ea`) — **store-freeing durable-sleep for Delay under the invoke export**: on a checkpoint MISS the Delay stamps its absolute deadline and EXITS with `outcome::suspended(at(deadline))` (freeing the `Store`); the environment stamps `status='suspended' + sleep_until=deadline`, the wake scheduler relaunches, and the replay HITS the deadline checkpoint and skips the sleep. The invoke launch path also resets `suspended → running` on relaunch so a woken instance's terminal event isn't dropped by its `if_running` guard. Slice 2b (this change) — **store-freeing WaitForSignal + the on-signal waker**: a durable Wait EXITS with `outcome::suspended(on-signal{signal-id, deadline?})` on the first poll MISS instead of blocking the poll loop; the environment parks it `suspended` (`sleep_until` = the timeout deadline, or NULL for a no-timeout wait) and a custom-signal submission stamps `sleep_until=now` (the waker in `handle_send_custom_signal`) so the wake scheduler relaunches it, whereupon the replay re-polls the now-present (non-destructively read) signal and completes. Both halves are **gated OFF by default** (`RUNTARA_DIRECT_STORE_FREEING_SLEEP=1`, compile-time): the default remains the byte-preserved blocking `durable-sleep-checkpoint` / poll loop, so short delays aren't pessimized by a suspend/relaunch — a threshold policy can later live behind the same gate. Live-e2e-verified: Delay suspends→wakes-at-deadline→completes; Wait suspends-on-signal→signal-submit→waker→wakes→completes, both byte-identical to the blocking output.
**Remaining follow-ups (deliberate, gated):** Phase 6 retirement (runtime crate / workflow HTTP backend / CLI axis / guest-protocol router) — gated on N green releases, MUST NOT co-land with the flip, per §5. A store-freeing THRESHOLD policy (block short delays, suspend long ones) remains open behind the same gate — the mechanism is in place; only the block-vs-suspend decision heuristic is deferred.
**Date:** 2026-07-14 · revised 2026-07-15
**Goal:** Replace the workflow guest's HTTP loopback to core (input / lifecycle / events / checkpoints / signals) with **host-function imports** satisfied natively in-process, and change the workflow's top-level export from `wasi:cli/run` to an **`invoke(input) → result<outcome, error-info>`** function shaped like an agent — so agents and workflows become the same kind of component.

---

## 0. TL;DR / verdict

The change splits into a **safe half** and a **risky half**, and they should ship separately:

- **Safe half (Phases 1–2): eliminate the HTTP loopback.** Make the `runtime` interface a host import (satisfied via `add_to_linker`, delegating to `runtara-core::instance_handlers` over `Arc<dyn Persistence>`), while keeping the `wasi:cli/run` export and blocking durable-sleep. This alone achieves *"workflows contact core via host functions, not HTTP"* with a low-risk, differentially-verifiable diff. **Recommended to land first and independently.**
- **Risky half (Phases 3–6): the `invoke` export + suspension-as-return-value.** This is where the real unification (agent == degenerate workflow) lands — and where verification found **two confirmed correctness blockers** and a test-harness blind spot. **UPDATE 2026-07-15: both blockers are FIXED on main** (A: `e8e663d9` non-destructive signal read + persisted wait deadline; B: `b1b28297` per-attempt retry durability). The risky half is now gated only on the three Phase-0 spikes, the on-signal waker, and the Delay sleep-key scoping (§3).

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
  variant outcome {
    completed(list<u8>),        // == the agent's ok-arm
    suspended(list<wake>),      // wake-SET: re-invoke when ANY condition fires.
                                // Sequential lowering emits singletons today; the set
                                // shape is deliberate — see §3 concurrency-forward note.
  }
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
- **Stage 2 (option-a, re-invoke):** flip the export to `invoke → outcome`; suspension points emit `Return suspended([wake])`; the runner tears down the `Store` and hands the wake conditions to the scheduler. This frees the `Store` during any wait — strictly better than today's resident poll loop *under the current sequential execution model* (see the concurrency-forward note below).

### Concurrency-forward design note (2026-07-15)

The suspension-as-return-value contract silently assumes **one program counter**: it is valid today only because the direct emitter *linearizes* all branches (fan-out executes sequentially in topological order — a wait in branch A blocks branch B by construction). Future parallelism changes the picture, and the contract must not bake the sequential assumption into the ABI:

- **Single-wake would break under any parallelism.** With N concurrently-pending waits (parallel branches waiting on different signals/timers, or a join over N children), the suspend payload must express a **set** of wake conditions with re-invoke-on-ANY semantics. Therefore `outcome::suspended` carries `list<wake>` from day one — the sequential emitter only ever produces singletons, so this costs nothing now and removes a guaranteed ABI break later. The host mirrors it: the Phase-4 signal-wait table is keyed `(instance, condition)` with N rows per instance from the start; multiple `at` deadlines collapse to `min → sleep_until` naturally.
- **Suspending mid-flight parallel work is a real semantic problem** the return-value model alone cannot solve: if branch A suspends while branch B is mid-agent-call, the host must either quiesce (delay the suspend until B reaches a checkpointed safe point; in-flight non-checkpointed work replays) or cancel B (risking duplicated side effects on non-idempotent calls). Any intra-instance parallelism design must specify the quiesce policy explicitly.
- **The likely futures both fit the set-shaped contract.** (a) *Host-orchestrated parallelism* — parallel regions compiled as separately-invokable units (natural under this unification: a region ≈ an embedded invoke component), each with its own Store, suspending independently; the join is an instance with a wake-set. Composes with this contract unchanged. (b) *Guest-side parallelism* (WASI p3 native async) — gives in-process concurrency (overlapping I/O without suspension) but does NOT replace durable suspension: surviving a process restart still requires quiescing to a replayable state, i.e. this same outcome contract. p3 is complementary, not competing.
- **Store-teardown-on-suspend is a scheduling policy, not an invariant.** Today (sequential) it is strictly better than the resident poll loop. Under parallelism, "tear down vs stay resident while sibling branches run" becomes a per-suspension host decision — keep that decision in the runner, out of the guest ABI.

### ✅ Two blockers that reshaped Stage 2 — both RESOLVED on main (2026-07-14/15)

**Blocker A — Wait-for-signal replay deadlock. FIXED in `e8e663d9`.** Was: custom signals destructively `DELETE`d on poll + wait never checkpointed → `[wait → later-suspension]` re-polled a deleted signal on replay and hung forever (or fired a spurious sliding `WAIT_TIMEOUT`). **As-built fix:** (1) `take_pending_custom_signal` is now a **non-destructive read** (Postgres `DELETE…RETURNING` → `SELECT`; SQLite drops the `DELETE`; rows reclaimed by `ON DELETE CASCADE` at instance deletion) — the pending-signal row is the durable record and replay re-reads it idempotently. Also fixes the AiAgent HITL tool arm (same op). (2) The wait's **absolute deadline is checkpointed** under the wait's deterministic signal id at first entry (`wait.rs:288/315`) and re-read on resume instead of recomputing `now+timeout`. **Consequence for this plan:** the wait poll loop can safely become a suspend point in Phase 4 — replay is an idempotent re-read, and `wake::at`/`on-signal.deadline-ms` can carry the *persisted* deadline. Fixtures `wait_delay_finish.json` + `wait_wait_finish.json` with simulated suspend/resume shipped with the fix.

**Blocker B — Retry-backoff side-effect amplification. FIXED for agent retry in `b1b28297`** (as-built: `docs/retry-backoff-durability-fix-plan.md`). Was: retry checkpointed only on success + attempt counter reset each entry → drain mid-backoff re-invoked every prior attempt. **As-built fix:** each **failed** attempt is checkpointed under `{cache_key}::attempt::{N}` (err-only envelope carrying the *already-computed* classification bits + raw retry-after; success rides the existing outer checkpoint); on replay a per-attempt hit short-circuits the invoke and the **backoff sleep is gated on the hit flag**. Loop-index folding in `cache_key` gives per-iteration isolation inside Split/While (guarded by a test). No runtime-WIT change (two new *stdlib* funcs: `agent-attempt-result-key`, `agent-attempt-envelope`). **Split/Embed retry deliberately deferred:** their retryable unit is a subgraph whose leaves carry their own per-step checkpoints — a drain-mid-backoff replay cache-hits the leaves, so no side-effect re-fire; an `::attempt::` envelope there is an optimization, not a correctness prerequisite. **Consequence for this plan:** all four `durable_sleep_checkpoint` callers can route through the suspend path in Phase 4 (agent backoffs replay via attempt-envelopes; split/embed backoffs replay via leaf checkpoints; Delay needs the key fix below).

**Still required for Stage 2 (the remaining open items):**
- **On-signal wake path is net-new** (MAJOR — still open). The wake scheduler only relaunches instances whose `sleep_until` is due (`wake_scheduler.rs:126`). A torn-down `on-signal` instance is never woken by a custom-signal insert. On custom-signal insert for a suspended instance, set `sleep_until=now` (reuse the wake scheduler) or add an explicit waker.
- **Durable-sleep Stage-2 is a tri-state, not a one-line `Return`** (MAJOR — still open). The host closure must be **non-blocking** and return `{continue | suspend(deadline)}`; the emitter needs a new decision point at the delay/durable-sleep site (`delay.rs:113` currently calls sleep then *unconditionally* continues).
- **Scope the Delay sleep-checkpoint key by loop indices** (upgraded to CONFIRMED — still open). Delay keys its durable sleep by **bare step-id** (`delay.rs:106–112`: step-id segment + two `I32Const(0)`, no loop indices) — the retry fix explicitly warns "do **not** copy that pattern." Latent today (HTTP `handle_sleep` re-sleeps the full duration regardless of key); becomes live under option-a per-item suspension where the key/deadline determines the wake.

**New fact that revises the sleep-semantics plan (verified `checkpoint.rs:245`):** core `handle_sleep` **unconditionally sleeps the full `duration_ms`** — the resume-remaining math exists *only* in the SDK embedded backend, which the WASM guest never uses. So the old path's observable behavior is *full re-sleep on replay*. Therefore: **Phase 2's host durable-sleep closure must mirror `handle_sleep` (full-duration) for differential parity — do not port the embedded remaining-math there.** The remaining-time correctness arrives naturally in Phase 4: under option-a the suspend carries the **absolute** deadline (`wake::at(deadline)`), so waking at the original deadline is automatic and no in-closure math is needed.

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
The safe-half payoff. `emit_wac`: drop the runtime `new`+spread (keep trailing `…` so runtime surfaces as a component import like WASI). Remove `workflow-runtime` from `DIRECT_SHARED_COMPONENT_REQUIREMENTS`. Wire the Phase-1 linker into the production execute path; thread `instance_id/tenant_id` via typed store state. Host implements **all 19** funcs incl. `load-input/complete/fail` (export is still `wasi:cli/run` this phase). **Durable-sleep closure mirrors `handle_sleep` semantics (full-duration re-sleep) for byte-parity with the old path — do NOT port the embedded backend's remaining-math here** (it would diverge in the parity harness; the absolute-deadline wake in Phase 4 supersedes it). **Acceptance:** compiled component now *lists* a `runtime` import; full 106-fixture A/B corpus green with the composed runtime removed; durable-sleep-across-simulated-restart, wait-for-signal delivery (incl. the new `wait_delay_finish`/`wait_wait_finish` fixtures), and the retry-resume acceptance tests from `b1b28297` all pass (blocking).

> After Phase 2 the HTTP loopback is gone. This is the shippable milestone for *"workflows use host functions, not HTTP."*

### Phase 3 — Invoke-export WIT + emitter variant (behind `WorkflowAbi` flag, default off) · risk: high
New workflow world v0.2.0: `export lifecycle.invoke(input)->result<outcome,error-info>`; keep `import runtime`. Update **both** emitters (`component.rs:120` `emit_world_wit` AND `compile.rs:836` `build_direct_component_resolve_with_agents`) in lockstep. Emitter `WorkflowAbi::{CliRunHttp default, InvokeHostImports}`. Under the new variant: (a) **fold the two invoke params onto locals 0/1** by shrinking the first declared-local group by two i32, so `DATA_PTR/DATA_LEN` land on param0/param1 and the ~100 `DIRECT_*_LOCAL` constants are unchanged; (b) read the input arg instead of `runtime.load-input`; (c) **write the outcome result variant** (fresh return-area offsets — the ok arm is a variant, *not* the agent read-offsets; err arm may coincide) via a single `emit_invoke_return_err` helper; (d) delete `load-input/complete/fail` from `require_all` + the index struct + call sites. Define a canonical `string → error-info` wrapping for non-structured failures (template/mapping/Filter errors reach `fail` as plain strings today, `compile.rs:879`). **Acceptance:** golden invoke-WIT snapshot; all 106 fixtures **compile** under the flag; a `compile::tests` assertion that the invoke body's param+local count is unchanged; **plus a focused output-ownership test** (the param-count assertion does NOT guard the return-area writer). Default path untouched.

### Phase 4 — Differential parity harness + suspend wiring · risk: high (behavior-changing)
Parametrize `direct_wasm_execute` over the ABI; add a capturing `add_to_linker` shim recording the same `{output,error,events,sleeps,checkpoints}` tuple the mock HTTP server records today. Generalize the early-return-to-suspend pattern (`checkpoint.rs:70`) to durable-sleep-checkpoint (as a **tri-state** `{continue | suspend(deadline)}`, per §3 — the host closure goes non-blocking here) and the wait poll loop (`Return suspended(wake)`, carrying the **persisted** deadline from `e8e663d9`). Add `WorkflowExit::Suspended` carrying the wake-set; route `at` → wake scheduler (multiple deadlines collapse to `min → sleep_until`), `on-signal` → **new host signal-wait table + custom-signal-insert waker** (the missing wake path) — keyed `(instance, condition)` with N rows per instance from day one (any-fires semantics, per the §3 concurrency-forward note). Re-fetch persisted enriched input on wake. **Scope the Delay sleep-checkpoint key by loop indices before enabling per-item suspension** (bare-step-id collision, §3).

> **Gate status (2026-07-15):** durable wait-result ✅ (`e8e663d9`), persisted wait deadline ✅ (`e8e663d9`), agent-retry per-attempt durability ✅ (`b1b28297`) — retry backoffs may now route through suspend. Delay key scoping ✅ (Slice 1 `29e86100`), **Delay store-freeing sleep tri-state ✅ built + gated OFF (Slice 2)** — a durable Delay under the invoke export emits `suspended(at(deadline))` and resumes via checkpoint-hit; environment `park_invoke_suspend` + relaunch `running` reset wired. **Wait store-freeing `suspended(on-signal)` + custom-signal waker ✅ built + gated OFF (Slice 2b)** — a durable Wait emits `suspended(on-signal{signal-id, deadline?})`, parks (`sleep_until` = timeout or NULL), and the `handle_send_custom_signal` waker stamps `sleep_until=now` to relaunch it. Only the block-vs-suspend THRESHOLD heuristic remains deferred. **Corpus status:** `[wait → durable delay → finish]` ✅, `[wait → wait]` ✅ (fix A), `[retry-resume + per-iteration Split isolation]` ✅ (fix B), `[split of N items each with a durable per-item delay]` ✅ (Slice 1 key fix), `[store-freeing Delay suspend→resume, byte-identical to blocking]` ✅ (Slice 2), `[store-freeing Wait suspend-on-signal→signal→resume]` ✅ (Slice 2b, in-process + live wake).

**Acceptance:** 106/106 differential parity old-vs-new on `{output,error,events,sleeps,checkpoints}` **plus** the post-suspension fixtures; under option-a a suspended durable sleep wakes at the **original absolute deadline** (`wake::at`); externally-delivered wait-for-signal is consumed by a host-import guest; split mid-iteration crash replays correctly; retry backoff fires only the frontier attempt across a resume (per `b1b28297`'s acceptance test).

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
| ~~Blocker~~ **RESOLVED** | Wait-for-signal replay deadlock (signals `DELETE`d on poll, wait not checkpointed) | ✅ `e8e663d9`: non-destructive signal read + deadline checkpointed under the wait's signal id |
| ~~Blocker~~ **RESOLVED** | Retry-backoff O(n²) side-effect amplification (success-only checkpoint) | ✅ `b1b28297`: per-attempt err-envelopes `{cache_key}::attempt::{N}`, backoff gated on hit; Split/Embed deferred (leaves self-checkpoint) |
| ~~High~~ **RESOLVED** | Wait absolute deadline resets on every re-invoke → never times out | ✅ `e8e663d9`: absolute deadline persisted at first entry, re-read on resume |
| High | Local-index rebase: invoke `input` occupies locals 0/1, shifting `DIRECT_*_LOCAL` indices (now ~106 after `b1b28297`'s locals 110–115) | Fold params onto 0/1 (shrink first local group by 2×i32 — verified still viable: `DATA_PTR/LEN` are locals 0/1, `core_module.rs:404`); assert param+local count unchanged |
| High | Return-area writer for `result<outcome,error-info>` is net-new (not verbatim agent-offset reuse) | Isolated spike (Phase 0.3) + golden output-ownership test before the 106-fixture corpus |
| High | Phase-2 sleep-semantics parity: `handle_sleep` re-sleeps the FULL duration (`checkpoint.rs:245`); porting embedded remaining-math would diverge in the parity harness | Phase 2 host closure mirrors `handle_sleep`; remaining-time correctness arrives via Phase 4's absolute-deadline wake (`wake::at`) |
| ~~High~~ **PARTLY DONE** | Parity harness blind spot: post-suspension fixtures missing | ✅ wait→delay, wait→wait, retry-resume, Split-isolation shipped with the fixes; still to add: split-with-per-item-durable-delay |
| High | Delay durable-sleep key is bare step-id → cross-iteration collision under per-item suspension (**CONFIRMED**, `delay.rs:106–112`; latent today because `handle_sleep` re-sleeps fully regardless) | Scope the Delay sleep key by loop indices (as the agent cache key does) before enabling option-a per-item suspension |
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
- **Per-iteration Split checkpoint:** under re-invoke suspension, a large Split with per-item waits re-runs items `0..suspended-iteration` each resume (O(n²) replay of cheap non-durable bodies). Is a per-iteration Split checkpoint needed to bound replay cost? (The Delay bare-step-id key collision that was flagged here is now CONFIRMED and promoted to a Stage-2 prerequisite — §3. Per-iteration *retry-attempt* isolation is already guaranteed and test-guarded by `b1b28297`.)
- **Outbound agent/LLM/object-model traffic stays on `wasi:http`** (confirmed scope: host imports cover input/lifecycle/events/checkpoints/signals only). The mock LLM proxy + object-model routes in the harness rely on this — confirm.
- **Final sweep:** does any non-guest tool (`runtara-ctl`, ops scripts) call the core guest-protocol port before it's gated off?
- **Should interruption rings (epoch/watchdog/memory limiter) apply to agent invokes once unified?** Agents run epoch `u64::MAX` today (`registry.rs:108`); per-tool-call timeouts on agents are desirable but a behavior change.

---

## 9. Escape hatch
**No standalone/out-of-process escape hatch is preserved.** The user ruled out the "workflow.wasm runs on stock wasmtime by calling our HTTP API" property (vendor protocol over a standard transport), and the out-of-process CLI process runner is **already removed** (`runner/mod.rs:22` resolves every `RUNTARA_RUNNER` value to the in-process `EmbeddedWasmRunner`). `RUNTIME_WIT` is deliberately retained in `runtara-workflow-wit` as the canonical host-interface contract, so re-adding an out-of-process runner later is one WAC `new`+spread line + one `DIRECT_SHARED_COMPONENT_REQUIREMENTS` entry + re-selecting the HTTP SDK backend — reversible even after the guest crate is retired.

---

## 10. Bottom line (updated 2026-07-15)
The core module already imports the `runtime` surface, persistence is already on hand where the executor is built, and wac already surfaces host-satisfied imports for WASI — so **Phases 1–2 (kill the HTTP loopback, host-import the runtime) are a clean, high-value, differentially-verifiable win worth shipping on their own.** **Both correctness blockers for the second half are now fixed on main** (`e8e663d9`: idempotent signal read + persisted wait deadline; `b1b28297`: per-attempt retry durability), and the post-suspension fixtures they shipped close most of the harness blind spot. What still gates option-a: the three Phase-0 spikes (wac unsatisfied-import, custom host-interface binding, invoke return-area lowering), the on-signal waker, the durable-sleep tri-state, and the Delay sleep-key loop-index scoping. One semantic pin from the fixes: Phase 2's host sleep closure must mirror `handle_sleep`'s full-duration behavior for parity — the remaining-time fix lands via Phase 4's absolute-deadline wake, not in-closure math. Sequence: spikes → Phase 1–2 (ship) → Phase 3 → Phase 4 (waker + tri-state + Delay key) → 5 → 6.
