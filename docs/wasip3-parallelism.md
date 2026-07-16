# In-Guest Workflow Parallelism via Component-Model Async (WASIp3)

**Status:** PHASES 0–3 LANDED on `worktree-wasip3-parallelism` (2026-07-16): Phase 0 `d43f556a`+`42d05179` (wasmtime 46 + Rust 1.97; cargo-component exit — all components on plain cargo + wit-bindgen 0.58). Spikes `82c92175`+`7f212d4d`+`d890279f` ALL PASS — including the decisive extra result that a **SYNC-ABI-lifted async-typed export drives waitable-sets with full overlap**, so no stackful lift or task-return was needed in production. Phase 2 `e5a422f4` (ABI v2: agent capabilities@0.4.0 + lifecycle@0.2.0 re-typed `async func`, sync lifts everywhere, dual-version executor lookup, battery 95/95). Phase 3 `ffad695d` (concurrent Split windows: launch/drain/assemble with consume-once memo slots, instance pooling ×4, and the §3.6 route (b) `runtara:host-io/http` concurrent hop for agents' proxied requests; battery 96/96 incl. an overlap e2e asserting ≥1.5x). Drain-loop pause/cancel polls + chunk-boundary suspend `b36c7c1f` (battery 97/97 incl. pause-mid-window→checkpoint-replay resume). Concurrent retry BACKOFF for BOTH non-durable AND durable retries `77f00d8b`+`854b9b21` (§3.4): retrying items back off as CONCURRENT timer subtasks (`runtara:host-io/timers`) instead of serializing in assemble; durable retries do the per-attempt `::attempt::N` checkpoint dance in the window so replay never double-fires — battery 100/100, incl. a non-durable overlap test (four 429→retry→200 items' backoffs land with a 0ms span), a durable overlap test (one `::attempt::1` checkpoint/item, zero durable sleeps), and a durable replay test (two runs on one checkpoint store re-fire ZERO agent calls). **Live-server-verified** (through the production compile/execute HTTP API): (a) Split parallelism=4 over 4× 1s http-agent calls — all four requests in the same second, wall 4.1s vs 7.4s sequential; (b) non-durable parallelism=4 over rate-limited (429→retry→200) http-agent calls — all four first calls AND all four backoff retries arrive with a **0ms span**, 8 total requests (2/item), all 200.
**Remaining (deliberate follow-ups):** durable-Split windows (persist/assemble against split-level checkpoints), streaming (as-completed) assembly, heterogeneous parallel branches, AiAgent parallel tool calls, parallel child workflows (§4.4 Phase 4), per-item launch/complete timestamps in step-debug events (per-step durations currently read as assembly-time for overlapped items), CI cli-axis job update for v2 artifacts.
**Date:** 2026-07-15 · updated 2026-07-16
**Provenance:** joint assessment of `~/work/wasip3/demo` (a working wasmtime-44 + component-model-async parallelism demo) against the direct emitter and runtime; ABI facts verified against the pinned crate sources (`wasmtime-44.0.2`, `wasm-encoder-0.247.0`, `wasmparser-0.246.2/0.247.0`, `wit-parser-0.247.0`, `wac-graph-0.10.0` in the local cargo registry), the component-model spec repo ([Explainer.md], [Concurrency.md], [CanonicalABI.md]), and wasmtime/wasm-tools release notes.
**Hard constraint:** host-orchestrated fan-out (an `invoke-batch` host import, host-side parallel regions with separate `Store`s) is **ruled out**. Parallelism must live in-guest: the compiled `workflow.wasm` owns its own concurrency, retry, and ordering semantics. The host may keep *mediating individual I/O calls* (the HostImport runtime binding, connection resolution) — that is not fan-out.

---

## 0. TL;DR / verdict

Component-model async gives us real in-guest parallelism — the demo proves N guest tasks blocked on host I/O genuinely overlap (`futures::join` over two plugins: ~600ms instead of ~1000ms wall-clock). Our runtime is one cargo feature and two `Config` flags away from it. The emitter is the entire cost center, and there is a hand-emitter-compatible path that avoids rewriting the code generator:

- **The stackful async lift** (`canon lift async` *without* `callback`) lets the emitted component keep its **single linear function body** and simply *block* in `waitable-set.wait` while async-lowered agent calls run as subtasks. No CPS/state-machine transform. The callback ABI (what wit-bindgen/rustc generate in the demo) would force exactly that transform and is the fallback, not the plan.
- **Build for the ratified ABI once (wasmtime 46+), not the 44-era draft.** wasmtime 44 accepts a shortcut (async-lowering imports whose WIT types are plain sync `func`, leaving agents untouched) — but wasm-tools 1.249 made the type-tie normative and wasmtime 46 enforces it *and* flips CM-async/WASI 0.3 on by default. Since host-orchestrated fan-out is off the table, there is no bridge worth shipping on the deprecated pattern: upgrade to 46 first and implement the final shape.
- **Two traps found in verification, both decisive for the design:**
  1. Blocking legality is keyed to the function **TYPE** (`async func`), not the lift ABI — a sync-typed export that blocks traps `CannotBlockSyncTask`. So `lifecycle.invoke` (and, transitively, the agent `invoke` import types) must be re-typed `async func`.
  2. `func_wrap_async` host imports **freeze the whole store** while pending (`fiber.rs` suspends with `KeepStore`); only `func_wrap_concurrent` — which requires the WIT function to be `async func` — yields task-level overlap. Today both our `runtime` interface *and* the p2 `wasi:http` binding that agents block on are store-freezing. Without solving the HTTP path, "parallel" agent calls would still serialize on their I/O.

Sequence: **Phase 0** (wasmtime 46 upgrade + cargo-component exit, both independently valuable) → **Phase 1 spikes** (stackful-lift PoC, HTTP overlap route, async-typed-WIT interop) → **Phase 2** (ABI v2: async-typed invoke/runtime/agent WIT, concurrent host rebinding, sequential semantics preserved, full A/B battery) → **Phase 3** (concurrent Split behind a `maxConcurrent` knob: windowed launch, waitable-set completion loop, per-item retry via timer subtasks, agent instance pooling) → **Phase 4** (streaming completion, heterogeneous parallel branches, AiAgent parallel tool calls, child-workflow concurrency).

---

## 1. Where we are today (facts, with sources)

### 1.1 The emitter emits one sequential instruction stream

- The direct emitter hand-emits the core module byte-by-byte via `wasm_encoder` — no rustc, no wit-bindgen (`crates/runtara-workflows/src/direct_wasm/compile.rs`, `compile/core_module.rs`). There is exactly **one real function body**; every step lowerer appends instructions into it (`compile/dispatcher.rs`).
- Fan-out is **topologically linearized** (`graph_order.rs`, `plan.rs:1370-1388`); a fan-out that never re-converges is rejected (E073). Split, While, and the AiAgent tool loop are sequential wasm `Loop`s (`compile/split.rs:517-524`, `compile/while_loop.rs:216-217`, `compile/ai_agent_loop.rs`).
- The calling convention assumes sequential execution: a **shared retptr scratch at offset 0** and a **shared agent-args scratch at offset 128** (`compile.rs:135`, low-memory layout comment at `compile.rs:150-152`), fixed shared locals for Split/While state (`compile.rs:194-276`, While deliberately aliases Split's registers), and a **bump allocator whose watermark is rewound each loop iteration** (`core_module.rs:368-422` `export_realloc`, `split.rs:137-160`) — the fix for the large-scope Split OOM.
- Agent components are **statically composed** into `workflow.wasm` via in-process wac-graph (`compile.rs:539-610`, `component.rs:191-224`); agent `invoke` is a plain synchronous cross-component core call (`compile/agent_invoke.rs`), bytes-in/bytes-out JSON.

### 1.2 The runtime host is nearly CM-async-ready

- Embedded runner: wasmtime **44.0.2**, features `[async, component-model, ...]` — **not** `component-model-async` (`crates/runtara-component-host/Cargo.toml`). One `Engine` per process, epoch interruption + tokio watchdog, **one `Store` per instance run** (`workflow.rs:391`), instances run concurrently via `tokio::spawn` (`runner/embedded.rs:513`).
- The `runtime` interface is a **host import** (default since the unify flip), bound function-by-function with `func_wrap_async` (`runtime_host.rs:201-389`) over `PersistenceRuntimeHost`.
- The default compile is the **fully-unified invoke shape**: `export runtara:workflow-lifecycle/lifecycle.invoke(input) -> result<outcome, error-info>` with `outcome::suspended(list<wake>)` — a wake-**set**, designed concurrency-forward (see `docs/unify-agents-workflows-plan.md` §3). Rollback levers exist per axis (`RUNTARA_DIRECT_WORKFLOW_ABI`, `RUNTARA_DIRECT_RUNTIME_BINDING`).

### 1.3 Durability is mostly concurrency-tolerant already

- Replay-from-start with a **key-addressed** checkpoint result cache: keys fold step id + loop indices (+ `::attempt::N` for retries). Key-addressed lookup is order-independent — out-of-order completion does not corrupt replay by construction.
- Landed since the unify follow-ups: Delay sleep keys are loop-index-scoped (Slice 1); **store-freeing durable-sleep** (Delay exits `suspended(at(deadline))`) and **store-freeing WaitForSignal + the on-signal waker** (Slice 2/2b) exist behind `RUNTARA_DIRECT_STORE_FREEING_SLEEP=1`.
- Still genuinely open for parallelism: the **quiesce policy** — what happens when the instance must suspend (drain/pause/store-freeing wait) while sibling subtasks are mid-flight (§4.4).

### 1.4 The verified CM-async fact base

| # | Fact | Consequence | Source |
|---|------|-------------|--------|
| 1 | Blocking legality is keyed to the function **TYPE**: a task for a non-`async func` export traps on `waitable-set.wait` (`CannotBlockSyncTask`; wasmtime keys `may_block` on `task.async_function`). An `async func` lifted with the **sync** ABI may block (holds an instance-wide lock); **stackful async lift** blocks freely with the highest concurrency. | `lifecycle.invoke` must be re-typed `async func`; the linear body survives via stackful lift. | Concurrency.md §Blocking; wasmtime `concurrent.rs:5143`, `trap_encoding.rs:149` |
| 2 | Async-**lowered** imports return `(subtask_handle<<4) \| state`; results are written through a **per-call retptr** when the subtask resolves; arg+result buffers must stay live until STARTED/RETURNED events arrive; completion is delivered as `SUBTASK` events (`{handle, state}` two u32s) via `waitable-set.{wait,poll}`; then `subtask.drop`. | Kills the shared retptr@0/args@128 scratch for anything in flight concurrently; per-call buffers required. | CanonicalABI.md §canon lower / flatten_functype; wasmtime `func/host.rs:479-600` |
| 3 | Concurrent calls into the **same** sync-lifted callee instance serialize (component-instance-wide lock, `do_not_enter`); N **distinct instantiations** of the same component are independent concurrency domains. wac-graph instantiates one package N times with the binary embedded once (N× linear memory, 1× code). | Same-agent overlap (the Split case) needs an **instance pool** in the composition, or async-lifted agents. | Concurrency.md §Backpressure; wasmtime `concurrent.rs:1696-1704`; wac-graph `graph.rs:769`, `encoding.rs:124` |
| 4 | `func_wrap_async` host functions suspend the fiber with `KeepStore` — **nothing else in the store runs while they're pending**. Only `func_wrap_concurrent` overlaps, and it typechecks the WIT function as `async func`. | The `runtime` functions used inside parallel regions AND the agents' HTTP path must move to `async func` + `func_wrap_concurrent`. | wasmtime `fiber.rs:264-299`, `linker.rs:430-524`, `func/host.rs:627` |
| 5 | wasmtime 44 accepts `async` canon options on sync-typed funcs (wasmparser 0.246/0.247 doesn't enforce the tie). **wasm-tools 1.249 (May 2026) requires `async func` types for async lift/lower; wasmtime 46 (2026-06-22) enforces it and enables CM-async + WASI 0.3.0 by default.** In 44 the feature is explicitly "*very* incomplete". | Build directly for 46+; do not ship the 44-era pattern. | wasm-tools v1.249.0 release notes (PR #2512); wasmtime v46.0.0 release notes (PR #13612); wasmtime 44 `config.rs:1195-1232` |
| 6 | Our pinned toolchain already has the full async surface: wasm-encoder 0.247 (`CanonicalOption::Async/Callback`, `task_return`, `waitable_set_*`, `waitable_join`, `subtask_drop`, `context_get/set`, `ComponentFuncTypeEncoder::async_`), wit-parser 0.247 parses `async func`, wac-graph 0.10 composes async **types** (validates with `WasmFeatures::all()`), async/sync func types are **invariant** under matching (a sync `func` export cannot satisfy an `async func` import). Caveats: verified against wasmtime **44**, not 46 (whose wasmparser is the post-1.249 0.251 line — S1 must prove 0.247-emitted bytes validate there); and our production composition goes through the **textual** `workflow.wac` script (wac-parser, whose p3 keyword support lags — wac issues #180/#210), not only the programmatic wac-graph API. | Emitting can start on the pinned line, but 46-validation and the textual-wac path are explicit spike exit criteria (S1/S3); contingency = wasm-tools line upgrade (no wac-graph release on ≥0.251 exists yet — 0.10.1 still pins 0.247). Agent WIT re-typing forces agent rebuilds (fact 5 + invariance). | wasm-encoder `canonicals.rs`; wac-types `checker.rs:174`; `compile.rs:15`, `component.rs` (`workflow.wac`) |
| 7 | Async-ness lives in the **component type** once declared on the WIT function, so re-typing `invoke` changes every composition edge that touches it; canon *options* alone change zero type bytes. | The re-typing (Phase 2) is one atomic ABI axis: workflow world + agent world + host bindings + emitter together. | Explainer.md functype `async?`; wac-types invariance |
| 8 | cargo-component is frozen (0.21.1, 2025-03-18, embeds wit-bindgen 0.41 / wasm-tools 0.227) and can never emit the ratified async ABI. wit-bindgen async ABI matching is by wasm-tools line: 0.53↔wasmtime 44 (the demo), **≥0.58↔wasmtime 46+**; 0.59 targets 47/trunk. | Exit cargo-component before any agent WIT goes async (Phase 0.2). | crates.io dependency pins; demo README pin warning |

---

## 2. Goals and non-goals

**Goals**
1. Overlap I/O-bound work inside one workflow instance: N Split iterations, parallel fan-out branches, and (later) parallel AiAgent tool calls / child workflows — bounded by an explicit concurrency knob.
2. Keep **all** orchestration semantics (ordering, retry, backoff, timeout, dontStopOnFailed, durability) in the emitted `workflow.wasm`.
3. Preserve the durable-execution contract unchanged: key-addressed replay, wake-set suspension, byte-preserved sequential output when the knob is off.
4. Land behind compile-time levers with full A/B parity against the sequential emitter, in the style of the unify migration.

**Non-goals**
- Thread-level parallelism / shared-memory threading (CM-async is cooperative, single-threaded per store — that is enough: our workloads are I/O-bound).
- CPU-bound parallelism inside one instance.
- Host-orchestrated fan-out in any form (hard constraint).
- Replacing durable suspension: per the unify plan §3, p3 async is **complementary** — overlapping I/O without suspension; surviving restarts still means quiescing to a replayable state.

---

## 3. Target architecture

### 3.1 ABI v2: the async-typed invoke component

One new value for the existing ABI axis: `RUNTARA_DIRECT_WORKFLOW_ABI=async-invoke`, alongside `cli-run` and the unset/default invoke shape. Note `workflow_abi_from_raw` (`compile.rs:817-823`) currently treats any unrecognized value as the default — introducing `async-invoke` must add an explicit match arm **and make unknown values loud**, otherwise setting the new value against an older binary silently compiles the sync shape.

```wit
// runtara:workflow-lifecycle@0.2.0 — same shape as @0.1.0, but the entry is async-typed.
interface lifecycle {
  // ... signal-wait / wake / outcome unchanged (wake-SET already concurrency-shaped) ...
  invoke: async func(input: list<u8>) -> result<outcome, error-info>;
}

// runtara:agent@0.4.0 capabilities — async-typed; otherwise byte-identical to @0.3.0.
// The connection stays IN-BAND (`_connection` inside `input`, opaque id, host-resolved) —
// there is deliberately no out-of-band connection argument (runtara-agent.wit:29-37,
// agent_invoke.rs:6-13); re-typing must not widen the credential boundary.
interface capabilities {
  invoke: async func(capability-id: string, input: list<u8>) -> result<list<u8>, error-info>;
}

// runtara:workflow-runtime@0.2.0 — the subset that can be awaited inside a parallel
// window goes async-typed (bound func_wrap_concurrent); see the dual-binding note in §5 Phase 2.
interface runtime {
  // async-typed → func_wrap_concurrent:
  durable-sleep: async func(ms: u64) -> result<_, string>;                    // TIMER SUBTASK (§3.4)
  durable-sleep-checkpoint: async func(checkpoint-id: string, state: list<u8>, ms: u64)
                            -> result<_, string>;                            // the keyed durable sleep — used by
                                                                              // Delay, rate-limited agent retries,
                                                                              // Split/Embed retries (§3.4)
  blocking-sleep: async func(ms: u64) -> result<_, string>;
  checkpoint: async func(...) -> result<checkpoint-result, string>;           // per-event persist (§3.2) must not freeze siblings
  get-checkpoint: async func(...) -> result<option<list<u8>>, string>;
  record-retry-attempt: async func(...) -> result<_, string>;
  poll-custom-signal: async func(...) -> result<option<list<u8>>, string>;
  check-signals: async func() -> result<bool, string>;                        // polled at completion-loop wakeups (§4.3)
  is-cancelled: async func() -> result<bool, string>;
  heartbeat: async func() -> result<_, string>;                               // emitted at loop granularity (§4.2)
  custom-event: async func(kind: string, payload: list<u8>) -> result<_, string>; // per-item debug events fire inside windows
  handle-checkpoint-signal: async func(signal-type: string) -> result<bool, string>;
  // sync-typed (genuinely never awaited inside a window):
  instance-id, now-ms, debug-mode-enabled, breakpoint-pause /* gated OUT of parallel windows, §3.7 */
}
```

- The **workflow export** is lifted with the **stackful async ABI** (`canon lift async`, no `callback`, no `post-return`): the emitted body stays one linear function, gains one `canon task.return` definition, and replaces "return result" with `call task.return; return`. Blocking in `waitable-set.wait` is legal because the *type* is async (fact 1).
- **Agent exports stay sync-implemented** where possible: an `async func` may be lifted with the sync ABI (fact 1) — the agent blocks holding its own instance lock, which is exactly the per-instance serialization we manage with pooling (§3.5). Whether wit-bindgen can express "async-typed WIT, sync Rust impl" is Spike S3; the fallback is trivially wrapping capability entry points in `async fn` with wit-bindgen `async: true`.
- The emitter **async-lowers** the agent `invoke` imports and the async-typed runtime imports. Canon options per fact 2: `async`, `memory`, `realloc` (results contain lists), utf8.
- Composed component type changes ripple (fact 7): workflow WIT, agent WIT, host `bindgen!`/dynamic bindings, dispatcher typechecks, and the `runtara:abi` drift-gate all move together — this is why Phase 2 is one atomic slice.

### 3.2 Emitter calling convention v2 (the core of the work)

**Per-in-flight-call buffers.** Each launched call owns, until its RETURNED event is consumed: an args struct, a retptr result buffer, and a slot record `{subtask_handle, state, step/iteration identity, retry state, deadline}`. Layout: a **call-slot table** in linear memory, bump-allocated per parallel region (N = window size), replacing offsets 0/128 *inside parallel regions only* — sequential lowering keeps today's scratch (byte-preservation when the knob is off).

**The completion loop.** Finalization is split into **two explicitly-ordered halves** so that durability is completion-ordered while results stay input-ordered and the arena discipline survives:

- **persist(i)** — runs *on each completion event, in completion order*: read the raw result via `slot[i].retptr`, decide retry-vs-settled (per-attempt `::attempt::N` err-only checkpoints as today), and on settle **durably checkpoint the raw item outcome under its existing key-addressed `{cache_key + loop indices}` key**. Key-addressed = order-independent (§4.1); a crash after persist(i) never re-runs item i.
- **assemble(window)** — runs *once per window, sequentially in input order*: agent-error envelope → bucket assignment (`dontStopOnFailed` semantics) → `stdlib` build-source append → then one watermark rewind + one `value-store-retain`. Assembly allocates only after all sibling item arenas are dead, preserving the rewind discipline; buckets/`stats`/downstream refs are byte-identical to the sequential lowering because assembly order is input order.

A parallel region compiles to:

```
region_setup:    ws = waitable-set.new
launch(i):       write args into slot[i]; r = invoke_lowered(argptr[i], retptr[i])
                 if r == RETURNED (eager): persist(i)
                 else: waitable.join(subtask(r), ws); slot[i].state = r
wait loop:       while live > 0:
                   ev = waitable-set.wait(ws, evptr)        // blocks; siblings + host futures progress
                   dispatch on (ev, evptr.handle):
                     SUBTASK RETURNED  -> persist(slot); subtask.drop; maybe launch(next)
                     timer RETURNED    -> retry-backoff elapsed -> relaunch attempt (§3.4)
                   signal/cancel poll at loop granularity (§4.3)
window_end:      assemble(window)                            // input order; then arena rewind + value-store-retain
region_teardown: waitable-set.drop(ws)
```

This is new codegen in the same style as the existing lowerers — `Block`/`Loop`/`BrTable` over event dispatch — not a new compilation model. `graph_order`/`plan` gain a `ParallelRegion` plan variant; `dispatcher.rs` routes to it only when the knob asks for it.

### 3.3 Memory strategy: windowed arenas first, streaming later

The per-iteration bump-watermark rewind (`split.rs:137-160`) assumes iteration i's allocations are dead before i+1 starts — false under concurrency. Two stages:

- **Phase 3 (windowed):** launch a window of K = `maxConcurrent` items; **persist** each item's outcome as it completes (§3.2); wait for the whole window; then **assemble** sequentially in input order and rewind the watermark + `value-store-retain` once per window. Bounded memory (K × item footprint), preserves the arena discipline, simple to verify. Cost: stragglers gate the window (overlap ≈ K-way, not perfectly pipelined) — but durability is never deferred to window end.
- **Phase 4 (streaming):** as-completed processing (`FuturesUnordered` semantics): fixed-size slab pool of K item-arenas recycled as each completes. Full pipelining; more allocator surface to test.

### 3.4 Retry, backoff, and timeouts become timer subtasks

**IMPLEMENTED for BOTH non-durable and durable retries** (`77f00d8b`+`854b9b21`, `split_parallel.rs`). Sequentially a retry backoff blocks in a host sleep: the **keyed `durable-sleep-checkpoint`** on the rate-limited path (`agent_retry.rs:114-159`) and plain `durable-sleep` otherwise. In a parallel window, an item's backoff is instead fired as a **concurrent timer subtask** (`runtara:host-io/timers.sleep`, async-lowered, joined into the window's waitable-set) — so all items' backoffs overlap. Between the launch drain and assemble, the window runs bounded retry ROUNDS: classify each agent result in place (reusing the exact `agent_retry` helpers via a slot↔retptr copy — same classification, delay, budget, error-routing as sequential), fire the eligible items' timers concurrently, drain them together, re-invoke concurrently, drain — until every item settles. Per-item retry state (attempt counter, rate-limit budget, prepared input, durable cache key) lives in the call slot; assemble then consumes the final post-retry result with retries disabled.

**Durable retries** additionally run the per-attempt `::attempt::N` checkpoint dance in the window so replay never double-fires: launch/re-invoke gate each attempt's invoke on `get-checkpoint(attempt::N)` (a HIT skips the invoke and its already-elapsed sleep — `SLOT_REINVOKE_NOW` — and classify decodes the stored envelope); a fresh MISS failure checkpoints its attempt envelope + records the audit row before the timer. Assemble runs `durable + max_retries=0` = step-gate + memo + step-save (no per-attempt loop), so a resume after full success HITs the step checkpoint at launch and re-fires nothing (proven by the `durable_backoff_replay_no_double_fire` test — two runs on one checkpoint store, zero re-fired calls). Eligibility gate: `concurrent_backoff = agent_retries > 0`.

**Interaction with `RUNTARA_DIRECT_STORE_FREEING_SLEEP` (not orthogonal):** with that lever ON, a durable-sleep MISS makes the guest exit `suspended(at(deadline))` — a whole-instance suspension that is illegal mid-window with siblings in flight. The lowering therefore forks on context: **inside a `ParallelRegion`, durable sleeps are always in-store timer subtasks regardless of the lever**; outside windows, the lever's policy applies unchanged. Long in-window backoffs holding the store resident is the accepted cost in Phase 3 (the drain policy in §4.4 bounds it).

Item timeouts are the same mechanism: a timer racing the agent subtask; loser is cancelled via `subtask.cancel` (or ignored-and-dropped if cancellation proves immature — decided in S1).

### 3.5 Same-agent overlap: instance pooling in the composition

Fact 3: one sync-lifted agent instance serializes concurrent entries. The wac composition (already ours, in-process) instantiates hot agents K times — `let agent-http-0 = new runtara:agent-http {...}; let agent-http-1 = ...` — and the emitter round-robins call slots across the pool imports. Pool size = `min(maxConcurrent, poolSize)` with `poolSize` derived per agent from the workflow's knobs (default: `maxConcurrent`, capped). Cost: K× that agent's linear memory (binary embedded once). Pooling is invisible to the DSL and to agent authors. If S3 lands async-lifted agents cheaply, pooling shrinks to a fallback for stateful/native-boundary agents.

### 3.6 The agent HTTP path (Spike S2 decides)

Two candidate routes for making the agents' actual I/O overlap-capable:

- **(a) wasi:http@0.3 on wasmtime 46** — if 46 ships a p3 http host, agents' shared client crate moves to the p3 interface (wit-bindgen async imports). Standard, but couples every agent to WASI 0.3 churn and to wit-bindgen async codegen.
- **(b) Host-mediated HTTP import** — a narrow `runtara:host-io/http.request: async func(req: list<u8>) -> result<list<u8>, error-info>` interface imported by agent components, left unsatisfied by wac and bound `func_wrap_concurrent`; the shared agent HTTP client crate targets it. Centralizes proxying/connection handling with the existing connection-proxy direction, keeps agents off wasi-http version churn, and is squarely "host mediates individual I/O calls" (allowed), not fan-out. Agents keep sync bodies: a sync-lowered call to an `async func` host import parks only the calling agent's task (fact 4 applies to the binding style, `func_wrap_concurrent` does not freeze the store).

Default assumption: **(b)**, validated or overturned by S2. Either way the change concentrates in the shared client crate + one rebuild sweep.

### 3.7 DSL surface

- `Split` gains `maxConcurrent: u32` (default **1** = today's sequential lowering, byte-preserved). `dontStopOnFailed`, `stats`/`hasFailures`, retry/timeout knobs keep identical semantics; result buckets keep **input order** (completion order is an implementation detail — required for replay determinism of downstream refs).
- Fan-out branches: a later `parallel: true` on the fan-out step (Phase 4), same windowed machinery over heterogeneous subgraphs. E073 (fan-out must re-converge) stays. **Detailed plan: [`docs/wasip3-parallel-branches-plan.md`](wasip3-parallel-branches-plan.md)** (new `ParallelBranches` plan node; phased 4a single-Agent branches → 4b linear-chain branches → 4c arbitrary subgraphs; opt-in, sequential fallback).
- Validation: initially `maxConcurrent > 1` is rejected (new W/E code) when the Split body contains suspension points — `Wait`, `Delay`, durable `Embed`/child-workflow invokes — **or breakpoint-bearing steps** (`breakpoint-pause` is sync-typed and store-freezing; pausing mid-window with K calls in flight is undefined until Phase 4 defines and tests it). The quiesce policy (§4.4) starts with the tractable case; the gate is lifted in Phase 4 with a pause-mid-window battery case.

---

## 4. Durability semantics under concurrency

### 4.1 What already holds
Key-addressed checkpoints make out-of-order completion safe: each item checkpoints under its existing `{cache_key + loop indices}` key; replay re-launches the region and every already-checkpointed item finalizes eagerly from the cache (HIT) without launching a subtask. `::attempt::N` and the backoff-gate rule survive unchanged (§3.4).

### 4.2 Event/summary ordering
Step-debug events inside a window interleave nondeterministically. Events already carry `scope_id`/`loop_indices` (the cartesian-product fix), so summaries key correctly; the verification battery must assert summaries are order-insensitive within a scope. Heartbeats: emitted at completion-loop granularity (each loop wakeup), plus the host watchdog remains the backstop.

### 4.3 Signals and cancellation (IMPLEMENTED as described here)
Sequentially, these polls exist only at While/Wait/Embed loop boundaries (`while_loop.rs:326,440`, `wait.rs:127,428`, `embed_workflow.rs:805`) — the sequential Split loop does **not** poll mid-loop. The parallel Split's drain loop *adds* a heartbeat plus `is-cancelled`/`check-signals` at each COMPLETION-EVENT wakeup (`split_parallel.rs`, gated off for `omit_runtime` compiles), a strict improvement over the sequential Split. Two deliberate properties:
- The wakeup polls only SET a sticky flag (`DIRECT_PSPLIT_SIGNAL_LOCAL`); the suspend itself fires at the **chunk boundary** — after every subtask has resolved and assemble has checkpointed durable items — via the same ABI-aware suspend-return the While loop head uses. Exiting mid-drain would tear down live subtasks; a poll ERROR also just sets the flag, and the boundary re-poll reports it with full error handling at a safe point.
- Staleness is bounded by one completion event: a window whose every launched call is stuck sees no wakeups and therefore no polls — the wall-clock rings (host-io timeout, epoch budget, watchdog) remain the backstop for the full-hang case. The polls are sync-typed `runtime@0.1.0` host calls, so in-flight host-io futures pause for the poll's duration (ms-scale, server-side rate-limited to 1s).

### 4.4 The quiesce policy (the one genuinely new semantic)
When the instance must stop mid-window (drain/shutdown signal, pause, or — Phase 4 — a store-freeing suspend raised by one branch while siblings are in flight):

> **Drain-then-suspend at window granularity** (implemented at chunk granularity in v1): stop launching new items; keep consuming completion events; run assemble so completed items persist their checkpoints; then exit through the standard suspend-return (`on-resume` wake). The battery's `parallel_split_pause_mid_window_resumes` case proves the contract end-to-end: a pause observed during the drain suspends at the boundary with all four launched calls resolved and checkpointed, and the resumed run replays entirely from checkpoints — zero re-fired agent calls. (Timer-parked retry items folding `at(deadline)` wakes into the exit set arrives with timer-subtask backoff, Phase 4.)

On resume, replay re-enters the region: persisted items HIT their checkpoints and finalize eagerly; an item whose backoff deadline passed relaunches its next attempt (the checkpoint-hit gate skips the elapsed sleep, exactly the sequential Blocker-B semantics). In-flight, non-checkpointed work at the moment of process death replays — the **same risk profile as today's** drain-mid-agent-call, widened to ≤ K calls. No cross-branch cancellation of already-launched work in Phase 3 (non-idempotent side effects are why); `subtask.cancel` for the timeout race is the only cancellation used. The wake-set contract needs no changes — it was designed for exactly this (unify plan §3, "suspended carries a wake-SET from day one").

---

## 5. Phases

### Phase 0 — independent prerequisites (shippable now, valuable standalone)

**0.1 wasmtime 46 upgrade** (embedder only). Sync agents and existing artifacts are unaffected (sync ABI stable; artifacts execute unchanged, as the unify flip demonstrated across ABI generations). Re-verify: epoch interruption + watchdog + `call_async` paths, wasi p2 bindings, full battery. Also confirms fact-5 enforcement behavior empirically (what 46 does with host-invoked sync-typed exports that block — flagged UNCONFIRMED in research).
**0.2 cargo-component exit** for the 28 component crates (26 agents + stdlib + workflow-runtime): delete committed `src/bindings.rs` + `[package.metadata.component]`, add `wit_bindgen::generate!` inside the same `mod bindings` (existing `use bindings::...` and `bindings::export!(... with_types_in ...)` wiring survives), swap `wit-bindgen-rt` → `wit-bindgen` (0.57 line now, ≥0.58 with 46), `cargo component build` → `cargo build --target wasm32-wasip2` in `scripts/build-agent-components.sh` (rustc's `wasm-component-ld` finalizes the component; no `wasm-tools component new` step). Pilot one agent (crypto), diff `wasm-tools component wit`, sweep, e2e-verify.
**0.3 Remaining durability groundwork:** none blocking — Delay key scoping, store-freeing sleep, and the on-signal waker landed with the unify slices. Note `RUNTARA_DIRECT_STORE_FREEING_SLEEP` is **not** orthogonal inside parallel regions: in-window durable sleeps are always in-store timer subtasks regardless of the lever (§3.4).

### Phase 1 — spikes (gate everything downstream)

| Spike | Question | Method | Exit criterion |
|---|---|---|---|
| **S1: stackful lift, hand-emitted** | Does wasmtime 46 correctly run a hand-emitted component: async-typed stackful-lifted export + `task.return` + N async-lowered calls + `waitable.join`/`waitable-set.wait` loop + per-call retptrs? Do bytes emitted with the **pinned wasm-encoder 0.247** and composed with **wac-graph 0.10** validate on 46's post-1.249 validator? Do epoch interruption, the watchdog, and cancellation still behave under the CM-async event loop? Does `subtask.cancel` work well enough for the timeout race? | New `spikes/` harness on **wasmtime 46**: emit the orchestrator AND the two plugins with wasm-encoder (plugins **async-typed, sync-lifted** — on 46 the type-tie forbids async-lowering sync-typed callees, so the demo's shape doesn't transplant; hand-emitting the plugins keeps S1 wit-bindgen-free). Assert wall-clock overlap and clean interrupts. | Overlap proven; 0.247-emitted bytes + wac-graph 0.10 composition validate on 46 (contingency: wasm-tools line upgrade — note no wac-graph release on ≥0.251 exists yet); interruption semantics documented; go/no-go on stackful vs callback fallback (§7), with the fallback's true cost priced (§7 row 1). |
| **S2: HTTP overlap route** | Does route (b) (host-mediated `async func` http import, `func_wrap_concurrent`) overlap two agents' requests? Is wasi:http@0.3 on 46 a viable (a)? How costly is one remaining **sync-typed** host call inside a window (store-freeze stall with K in-flight HTTP futures)? | Two sync agents in one composition issuing real HTTP against a local stub; measure wall-clock under both routes; measure the sync-call stall to validate §3.1's async/sync subset split. | One route selected with timing evidence; the runtime-interface subset split confirmed or amended by measurement. |
| **S3: async-typed WIT interop** | Can current wit-bindgen sync-implement an `async func` export (sync lift of async type)? If not, cost of `async: true` agent builds on 46 (≥0.58)? Does the **production textual `workflow.wac` pipeline** (wac-parser → wac-resolver → wac-graph, `compile.rs:15`) round-trip async-typed worlds — wac-parser's p3 keyword support lags (wac #180/#210)? | Re-type one agent's WIT, build both ways, compose **through the production wac path**, execute. | Agent migration recipe fixed (sync-impl vs async-wrapper); textual-wac round-trip proven (fallback: switch composition to the programmatic wac-graph API, or patch wac-parser). |

### Phase 2 — ABI v2, sequentially-equivalent (one atomic slice, gated)

New axis value `RUNTARA_DIRECT_WORKFLOW_ABI=async-invoke`, default OFF (with an explicit match arm + reject-unknown, §3.1):
- WIT: lifecycle `@0.2.0` (`invoke: async func`), agent capabilities `@0.4.0` (`async func`, connection stays in-band), runtime `@0.2.0` split into async-typed/sync-typed subsets (§3.1); `runtara:abi` drift-gate extended to the new versions.
- **Dual runtime binding (required for "old artifacts keep executing"):** async/sync func types are invariant (fact 6), so one linker definition cannot satisfy both a pre-v2 artifact's sync-typed `runtara:workflow-runtime@0.1.0` import and a v2 artifact's async-typed `@0.2.0` import. The host binds **both package versions simultaneously** (distinct import names): `@0.1.0` keeps its `func_wrap_async` bindings verbatim; `@0.2.0` binds the async subset with `func_wrap_concurrent`. Old artifacts link the old instance, new ones the new — no per-artifact linker selection needed.
- **Closure-wide ABI (stored artifacts):** the axis covers every *composed* component, not just repo agents. Two stored classes exist beyond the 26 in-repo agents: published **workflow-as-agent** artifacts (slug-imported by parent compiles) and **child-workflow** artifacts (`runtara-compile --child` / closure validation). A sync-typed stored export cannot satisfy a v2 parent's async-typed import (fact 6) — so: parent compiles verify axis homogeneity across the closure (new E-code for mixed-ABI closures), artifacts are stamped with their axis, and published slugs get a server-side **recompile-at-axis** path (their DSL sources are stored, so bulk republish is mechanical).
- Emitter: stackful lift + `task.return` for the export; async-lower agent + async-typed runtime imports **but keep fully sequential semantics** — launch one subtask, immediately `waitable-set.wait` for it. Behavior-identical to the sync invoke shape by construction.
- Host: enable `component-model-async` cargo feature + `Config::wasm_component_model_async(true)` + `..._stackful(true)`; executor drives the export via the concurrent call path; watchdog/epoch re-verified (from S1).
- Agents: WIT re-typing + rebuild per the S3 recipe (mechanical sweep; Phase 0.2 made this possible). The standalone agent dispatcher / `test_capability` path re-typechecks against `@0.4.0` (its `bindgen!` world and dynamic typechecks move with the WIT).
- Verification: the full A/B battery (three axes, 77/77 style) — v2-sequential vs the sync invoke shape must be observably identical (outputs, checkpoints, events, suspension behavior); old artifacts (both prior ABI generations) keep executing unchanged; mixed-closure rejection tested.

### Phase 3 — concurrent Split (the payoff)

Behind `Split.maxConcurrent` (default 1 → byte-preserved sequential path):
- `ParallelRegion` plan variant + windowed completion-loop codegen (§3.2–3.3), call-slot table, per-item retry/timeout via timer subtasks (§3.4), agent instance pooling (§3.5).
- Quiesce policy §4.4; validator gate excluding suspension-point bodies; per-item events with scope ids; `stats`/`hasFailures`/bucket semantics identical; input-ordered results.
- Verification: hermetic slow-agent stub (the LLM-stub pattern) with deliberate latencies; assert (i) wall-clock ≈ max not sum, (ii) results/buckets identical to sequential run, (iii) replay after kill-mid-window completes with no double side effects (idempotency ledger on the stub), (iv) drain-mid-window suspends and resumes correctly, (v) summaries order-insensitive. A/B against `maxConcurrent=1` on the whole battery.

### Phase 4 — breadth

- **Streaming completion** (as-completed slab arenas) replacing windowed joins where profitable.
- **Parallel fan-out branches** (heterogeneous subgraphs between fan-out and merge; E073 unchanged).
- **AiAgent parallel tool calls** (the tool-dispatch inner loop fans out a turn's calls into one window).
- **Child workflows / Embed in parallel windows**, including suspending children: a child returning `suspended(wake-set)` inside a window folds its wakes into the region's wake-set — this is where the wake-set's list shape earns its keep; lifts the Phase-3 validator gate.
- Threshold policies: auto-derive pool sizes; `maxConcurrent` defaults per plan tier.

---

## 6. What is explicitly NOT changing

- The DSL reference/mapping model, checkpoint key scheme, `::attempt::N` retry durability, wake-set contract, and the management/control plane.
- Sequential lowering: `maxConcurrent=1` (and every non-Split step) keeps today's codegen byte-for-byte per axis — the A/B lever stays meaningful.
- The composition model: agents remain statically composed through the in-process wac pipeline (textual `workflow.wac` → wac-parser/wac-resolver/wac-graph — whose async round-trip S3 proves); the host keeps satisfying only WASI + `runtime` (+ `host-io` if S2 picks route (b)).
- Native-boundary agents (sftp/xlsx/compression) and the credential boundary: connection info stays an opaque id; host-side resolution unchanged.

---

## 7. Fallbacks and kill criteria

| Risk | Signal | Fallback |
|---|---|---|
| Stackful lift broken/immature on 46 (S1 fails) | traps, lost events, deadlocks in the PoC | **Callback ABI — a whole-body restructuring, priced honestly:** under the callback ABI, blocking means *returning* from the core function (`WAIT\|set<<4`) and re-entering via the callback entry (`(event, index, payload) -> code`). A parallel region sits mid-body, often nested inside While/Split `Loop`s whose state lives in fixed shared locals (§1.1) — you cannot re-enter into the middle of a wasm loop, so every region boundary forces (a) spilling **all** live state, including enclosing loop counters/cursors, to linear memory (+`context.get/set`), and (b) restructuring the body into a top-level resumption dispatch over region boundaries. Sequential segments can keep sync-lowered imports (legal from an async-typed task), so only region boundaries yield — but this is a state-machine transform of the whole body at region granularity, not a change confined to the region lowerer. S1's go/no-go must weigh this cost explicitly; if it comes to this, re-evaluate scope before committing. |
| Neither wasi:http@0.3 nor host-mediated http viable (S2 fails) | no overlap through agents' I/O | Parallelism ships for host-import-bound waits only (sleeps/timers — nearly worthless) → **pause the initiative after Phase 2**; the ABI v2 work still stands (it's the 46-native shape regardless). |
| wit-bindgen can't sync-implement async-typed exports (S3 partial) | build failures on re-typed agent WIT | Agents move to `async fn` bodies via `async: true` (mechanical wrapper; no logic change). |
| Windowed arena still leaks/regresses memory | battery memory assertions | Reduce default window; per-item slab pool earlier (pull Phase-4 allocator forward). |
| wasmtime 47+ ABI drift (we now own a hand-emitted async ABI) | validation/trap changes on upgrade | Treat the S1 harness as a **permanent conformance test** in CI (like the wac-smoke suite); pin upgrades behind it. |

---

## 8. Open questions

1. `subtask.cancel` maturity on 46 for the timeout race (S1) — else timeouts fall back to "ignore-and-drop, item arena retained until region end".
2. Backpressure builtins (`backpressure.inc/dec`) for pool admission vs pure emitter-side windowing (start with windowing; revisit in Phase 4).
3. Pool sizing defaults and memory ceilings per entitlement tier (interacts with the per-instance `ResourceLimiter`, default 1 GiB).
4. Whether `checkpoint`/`get-checkpoint` staying briefly store-freezing (sync-typed) is acceptable in practice vs re-typing them async in Phase 2 (§3.1 currently re-types them — measure in S2's harness).
5. Migration marketing: `maxConcurrent` exposure in the Step Picker / builder UI (out of scope here; needs the schema regen flow).

[Explainer.md]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
[Concurrency.md]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[CanonicalABI.md]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
