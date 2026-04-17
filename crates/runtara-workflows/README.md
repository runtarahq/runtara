# runtara-workflows

Compiles runtara DSL scenarios into WASM components that run in wasmtime.

## What it is

A compilation library and CLI that turns a `runtara-dsl` `Scenario` (JSON) into a `wasm32-wasip2` component which talks to `runtara-core` over the SDK for durability, checkpointing, and signals. The pipeline is: parse DSL, resolve child-scenario and agent dependencies, generate Rust AST via `codegen`, write a source tree, invoke `rustc`, and optionally package the artifact. The target is runtime-selectable via the `RUNTARA_COMPILE_TARGET` env var and defaults to `wasm32-wasip2`; a musl fallback path exists but is vestigial. The crate exposes `compile_scenario`, `translate_scenario`, `validate_workflow`, and the `CompilationInput` / `NativeCompilationResult` types; it has no database dependencies and expects callers to resolve and pass in child scenarios.

## Using it standalone

The `runtara-compile` binary compiles a workflow JSON to a `.wasm` artifact. Requires `rustc` with the target installed (`rustup target add wasm32-wasip2`).

```bash
cargo install --path crates/runtara-workflows
runtara-compile \
  --workflow workflow.json \
  --tenant acme \
  --scenario order-sync \
  --output ./order-sync.wasm
```

Other useful flags: `--validate` (no compilation), `--analyze` (report only), `--emit-source <path>` (dump generated Rust), `--debug`, `--verbose`. Override the target with `RUNTARA_COMPILE_TARGET=...`. Build artifacts live under `$DATA_DIR` (default `.data`).

## Inside Runtara

- Primary consumer: `runtara-server` — `src/compiler/` and the scenarios API (`api/services/compilation.rs`, `api/services/scenarios.rs`) drive compilation on scenario create/update.
- Also consumed by `runtara-connections` for connection-bound scenario compilation paths.
- Upstream deps: `runtara-dsl` (scenario/execution-graph types, re-exported), `runtara-agents` (capability inventory linked at validation time), `runtara-ai`.
- Key integration point: `compile::compile_scenario` — the server calls it after a `ChildScenarioInput` list is resolved; the result is an artifact path plus metadata the dispatcher registers for execution.
- This crate is a host-side build tool (runs on native host). The *output* runs as a WASM guest inside wasmtime on the workflow-instance side.

## License

AGPL-3.0-or-later.
