# Spike S3 — wit-bindgen sync-impl of an async-typed export

Answers the ABI v2 agent-migration question from `docs/wasip3-parallelism.md`:
**can current wit-bindgen sync-implement an `async func` export?**

Yes — `wit_bindgen::generate!({ ..., async: false })` on wit-bindgen 0.58:
- keeps the WIT type `run: async func(ms: u64) -> u64` in the produced
  component (`wasm-tools component wit`),
- emits a plain **sync** `canon lift` (legal: an async-typed function may be
  lifted with the sync ABI; the callee blocks holding its instance lock),
- validates under the post-1.249 rules wasmtime 46 enforces,
- and the Rust impl stays a plain `fn` — **zero body changes per agent**.

Default (no `async:` option) generates an async trait method instead, so the
sweep must add `async: false` explicitly.
