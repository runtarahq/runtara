# Runtime-linked workflow stdlib

Status: implemented; linked is the default. The phases and what was built
are in `docs/stdlib-runtime-linking-plan.md`.

## Why

Every direct workflow is composed (wac-graph) into one component that embeds
the shared stdlib, the `runtara:api` component and the run shim. wasmtime
compiles a component as one unit and caches it by its full bytes, so each
workflow recompiles the identical stdlib:

| Piece of `conditional_workflow` | Size | Cranelift CPU |
|---|---|---|
| `runtara_workflow_stdlib` | 4.39 MB | 1.95 s |
| `runtara_api_component` | 932 KB | 0.45 s |
| `runtara_workflow_api_run` | 412 KB | 0.22 s |
| emitted logic | 72 KB | ~0.09 s |
| composed `workflow.wasm` | 5.8 MB | 2.70 s |

This is most of the CPU in the `direct_wasm_execute` CI step and of every
deploy's compile latency.

## What the spike showed

Composition leaves the stdlib out; the host compiles it once and links it into
each run's Store.

- wasmtime 50 has no API that hands one component instance's exports to
  another component as imports; the host must forward each call.
- On a concurrency-enabled Store (required by the p3 workflow API), a
  synchronously typed import cannot call into another guest instance:
  `func_new` + `Func::call` is rejected (store needs `*_async`),
  `func_new_async` + `call_async` is a recursive `run_concurrent`, a spawned
  `call_concurrent` task hangs, and `func_wrap_concurrent` is refused for
  non-`async` imports.
- What works: the logic imports `runtara:workflow-stdlib/json` with every
  function typed `async func` (sync-lowered, so the emitted core ABI and the
  artifact bytes are unchanged); the host implements each with
  `func_wrap_concurrent` and calls the stdlib export with
  `TypedFunc::call_concurrent`. The stdlib itself is unchanged.
- The import type is a one-way door: a sync export cannot satisfy it, so there
  is no fused fallback for the same WIT.

Measured (16-core arm64, test profile with `runtara-component-host` at
opt-level 3):

| | Fused | Linked, typed forwarder |
|---|---|---|
| Artifact per workflow | 5.8 MB | 1.43 MB |
| Cranelift per workflow | 2.70 CPU-s | 0.74 CPU-s |
| `direct_wasm_execute`, wall / CPU | 273 s / 3,362 s | 95 s / 1,102 s (42 known failures, below) |
| `split_large_scope_does_not_exhaust_guest_heap` exec | 494 ms | 363 ms |
| `split_large_scope_with_events_does_not_flood_debug` exec | 18 ms | 15 ms |
| `while_growing_accumulator_stays_bounded` exec | 1,635 ms | 938 ms |

The big-payload tests move up to ~154 MB per run across the boundary
(`build-source` in, `split-iteration-variables` out). Two cautions:

- A dynamic (`Val`) forwarder is 5–30x slower on those tests: it boxes every
  byte of a `list<u8>`. Only the typed path is viable.
- Generic lift/lower code is compiled in the calling crate. With
  `runtara-component-host` at opt-level 0 the typed path copies byte by byte
  and loses to fused; test builds need that crate optimized.

Why typed beats fused alone is not explained, but it is bounded: in the typed
runs 92% of execution is inside forwarded stdlib calls (335 of 363 ms, 860 of
938 ms), so the difference lies in the copies or the stdlib code, not the
logic. Summed execution under 16-way load is higher linked (37 s vs 22 s).

The 42 linked failures are a subset of the dynamic run's 43 and all trace to
shared stdlib state (below); the typed path never hit wasmtime's host-copy
budget, which the dynamic path did once.

## Proposed structure

- Per boot: the stdlib goes through the existing precompile worker
  (`--internal-precompile-component`); the parent keeps the `Component`, and
  the wasmtime disk cache makes later boots near-free. Shipping a `.cwasm` in
  the bundle would pin the wasmtime version, target, compiler flags, tunables
  and a CPU-feature baseline for no gain.
- Per workflow version: compose logic + api + run shim only.
- Per run: one stdlib instance in the Store, and one context per logic
  instance (the root and each published workflow-agent).

### Contexts

The stdlib is stateful per logic: the parsed manifest (step configs by numeric
id), compiled-condition cache, `local_states`, debug start times, and the
value-store arena with its per-run handle nonce. Fused, every logic gets its
own stdlib instance; linked, they share one, and the spike's 42 failures are
parent and child clobbering each other's manifest ("unknown direct Agent id 0").

Two stages:

- **Stage A: context id.** `init-manifest` returns a context id and every
  other function takes it first. The stdlib keeps one slot per context;
  handles carry the context's nonce. One stdlib instance per run, no per-site
  import names. The scope stays in logic memory as JSON bytes, so checkpoint
  keys, payloads and replay are unchanged. This fixes the spike's failures.
- **Stage B: scope in the context.** The context owns the scope as parsed
  values and only ids cross the boundary (today 78 of 138 functions take the
  scope as JSON and re-parse it). Larger, and the only stage that needs the
  checkpoint design below.

## Checkpointing with a stdlib-held scope (stage B)

Durability is replay from start over memoized step results. Checkpoints hold
values keyed by step: Agent/EmbedWorkflow outputs, Split rows, a While's final
`{iterations, outputs}`, GetState reads, local SetState `{patch, state}`
records. No checkpoint holds the scope or any stdlib state, and a parked run
drops its Store and replays. So:

1. A context is ephemeral, like the scope bytes in logic memory today. It is
   never persisted; replay rebuilds it.
2. Checkpoint payloads stay plain JSON values. Before a save, the logic asks
   the stdlib to export a value (materializing every `$wfref`); handles and
   context ids never reach a checkpoint.
3. On a hit, the logic hands the stored bytes back to the stdlib, which
   installs them in the context's scope. Local state keeps using
   `state-local-restore`, now per context.
4. The logic keeps all host I/O (`runtara:api` checkpoints, signals, time).
   The stdlib stays pure and imports nothing but WASI.
5. Keys are computed by the stdlib from the scope (`agent-cache-key`,
   `split-cache-key`, `loop-deadline-key`, `state-key`, `ai-turn-*`, …). From a
   context they must stay byte-identical: a run parked before an upgrade must
   hit its rows, and scope collapse matches addresses derived from the
   canonical `runtara:v2:` grammar (`docs/checkpoint-scope-collapse.md`).
6. Replay correctness still rests on the stdlib being deterministic for the
   same inputs, as today.

## Versioning

Today each artifact pins its stdlib bytes. Linked, every compiled workflow
depends on the stdlib WIT at run time, so either the interface stays
backward-compatible, or the server keeps each stdlib version an artifact was
built against (by package version or digest) and links that one.

## Open items

- Per-logic contexts in the stdlib and emitter (fixes the 42 failures).
- Typed forwarders for all 65 signature shapes (the spike typed 20 shapes,
  86 of 138 functions; the rest stayed dynamic). Generating them with
  `bindgen!` from the stdlib WIT avoids a hand-kept table.
- Why execution under load is slower in aggregate.
- The opt-level effect applies to every typed host function already in
  `runtara-component-host` (workflow API, control, database); whether it
  shows in today's test times is unmeasured.
- Remaining spike wiring gaps (the capability-run path).
- A stdlib version policy.

The test-only per-function compile cache (`Config::enable_incremental_compilation`)
is independent and cut the same suite's CPU by 56%.
