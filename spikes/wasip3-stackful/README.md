# Spike S1 — stackful async lift, hand-emitted (wasip3 parallelism plan)

Proves the load-bearing bets of `docs/wasip3-parallelism.md` Phase 1:

1. A component whose core module is **hand-emitted with the production pins**
   (wasm-encoder / wit-parser / wit-component **0.247**) can use the
   **stackful async lift** (`[async-lift-stackful]` export mangling, no
   callback, results via `[task-return]`) plus **async-lowered imports**
   (`[async-lower]` field prefix, per-call retptrs) and a
   `waitable-set` completion loop — and it **validates and runs on
   wasmtime 46** (post-1.249 validator).
2. The **production textual-wac pipeline** (wac-parser / wac-resolver /
   wac-graph 0.10) composes async-typed worlds unchanged.
3. **Real overlap**: two sync-lifted (async-TYPED) plugin components blocked
   in a `func_wrap_concurrent` host sleep progress concurrently when driven
   as subtasks — the exact agent execution model of ABI v2.
4. Epoch interruption (the production interruption ring) coexists with the
   CM-async event loop.

## Run

```bash
cargo run   # standalone crate, not a workspace member
```

Expected output shape:

```
[run-seq ] sum=300 wall=~300ms   # sequential baseline: a then b
[run-both] sum=300 wall=~150ms   # both subtasks in one waitable-set
PASS: overlap proven
```

## Key mechanics (mirrors what the direct emitter will do in Phase 2/3)

- Import `run` async-lowered: module `demo:plugins/alpha@0.1.0`, field
  `[async-lower]run`, core `(i64 ms, i32 retptr) -> i32 status`;
  status low 4 bits (`RETURNED=2`), high bits = subtask handle.
- Builtins from `$root`: `[waitable-set-new]`, `[waitable-set-wait]`,
  `[waitable-set-drop]`, `[waitable-join]`, `[subtask-drop]`.
- Export `[async-lift-stackful]demo:app/runner@0.1.0#run-both`, core
  `(i64) -> ()`; completion via import
  `[export]demo:app/runner@0.1.0` / `[task-return]run-both` `(i64) -> ()`.
- `waitable-set.wait` writes `{handle: u32, state: u32}` at the event
  pointer; the loop drops each subtask on `state == RETURNED`.
- wit-parser 0.247 layout gotcha: see `tests/wit_probe.rs`.

Still open here (tracked in the plan): `subtask.cancel` for the timeout
race, and the sync-typed host-call stall measurement (S2).
