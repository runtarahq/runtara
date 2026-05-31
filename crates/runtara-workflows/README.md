# runtara-workflows

Compiles runtara DSL workflows into WASM components that run in wasmtime.

## What it is

A compilation library that turns a `runtara-dsl` `Workflow` (JSON) into a
`wasm32-wasip2` component which talks to `runtara-core` over the SDK for
durability, checkpointing, and signals. Compilation runs fully in-process: the
direct WASM emitter byte-emits the workflow-logic module from the typed DSL and
composes the final `workflow.wasm` against the shared agent/stdlib/runtime
components via the `wac-graph` Rust crate — no `rustc`, `cargo-component`, or
`wac` CLI is shelled out. The target is runtime-selectable via the
`RUNTARA_COMPILE_TARGET` env var and defaults to `wasm32-wasip2`. The crate
exposes `compile_workflow`, `translate_workflow`, `validate_workflow`, and the
`CompilationInput` / `NativeCompilationResult` types; it has no database
dependencies and expects callers to resolve and pass in child workflows.

## Inside Runtara

- Primary consumer: `runtara-server` — `src/compiler/` and the workflows API (`api/services/compilation.rs`, `api/services/workflows.rs`) drive compilation on workflow create/update.
- Also consumed by `runtara-connections` for connection-bound workflow compilation paths.
- Upstream deps: `runtara-dsl` (workflow/execution-graph types, re-exported), `runtara-agents` (static capability registry linked at validation time), `runtara-ai`.
- Key integration point: `compile::compile_workflow` — the server calls it after a `ChildWorkflowInput` list is resolved; the result is an artifact path plus metadata the dispatcher registers for execution.
- This crate is a host-side build tool (runs on native host). The *output* runs as a WASM guest inside wasmtime on the workflow-instance side.

## License

AGPL-3.0-or-later.
