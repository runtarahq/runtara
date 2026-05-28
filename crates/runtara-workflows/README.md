# runtara-workflows

Compiles runtara DSL workflows into WASM components that run in wasmtime.

## What it is

A compilation library and CLI that turns a `runtara-dsl` `Workflow` (JSON) into a `wasm32-wasip2` component which talks to `runtara-core` over the SDK for durability, checkpointing, and signals. The pipeline is: parse DSL, resolve child-workflow and agent dependencies, generate Rust AST via `codegen`, write a source tree, invoke `rustc`, and optionally package the artifact. The target is runtime-selectable via the `RUNTARA_COMPILE_TARGET` env var and defaults to `wasm32-wasip2`; a musl fallback path exists but is vestigial. The crate exposes `compile_workflow`, `translate_workflow`, `validate_workflow`, and the `CompilationInput` / `NativeCompilationResult` types; it has no database dependencies and expects callers to resolve and pass in child workflows.

## Using it standalone

The `runtara-compile` binary compiles a workflow JSON to a `.wasm` artifact. Requires `rustc` with the target installed (`rustup target add wasm32-wasip2`).

```bash
cargo install --path crates/runtara-workflows
runtara-compile \
  --workflow workflow.json \
  --tenant acme \
  --workflow order-sync \
  --output ./order-sync.wasm
```

Other useful flags: `--validate` (no compilation), `--analyze` (report only), `--emit-source <path>` (dump generated Rust), `--debug`, `--verbose`. Override the target with `RUNTARA_COMPILE_TARGET=...`. Build artifacts live under `$DATA_DIR` (default `.data`).

## Direct Wasm PoC

The `runtara-direct-wasm-poc` binary experiments with emitting a core Wasm module directly from the typed DSL, without generating a Rust crate or invoking `cargo component build`. This is not the production workflow ABI. It currently lowers a small control-flow subset (`Finish`, `Log`, and `Conditional` over `data.flag`) into `run_bool(flag: i32) -> finish_code`, and writes unsupported DSL features plus finish mappings into a `runtara.direct_wasm_poc` custom section.

```bash
cargo run -p runtara-workflows --bin runtara-direct-wasm-poc -- \
  --workflow crates/runtara-workflows/tests/fixtures/conditional_workflow.json \
  --output /tmp/runtara-direct-poc.wasm \
  --compare-rust-codegen
```

Use this to compare direct Wasm emission cost and size against the current Rust/component artifact codegen before designing a production direct-emission ABI for JSON mapping, agent dispatch, durability, and component-model exports.

Add `--full-rust-compile` when you also want to invoke the existing `cargo component build` plus `wac compose` pipeline; that requires the same local tools and agent components as production compilation.

## Inside Runtara

- Primary consumer: `runtara-server` — `src/compiler/` and the workflows API (`api/services/compilation.rs`, `api/services/workflows.rs`) drive compilation on workflow create/update.
- Also consumed by `runtara-connections` for connection-bound workflow compilation paths.
- Upstream deps: `runtara-dsl` (workflow/execution-graph types, re-exported), `runtara-agents` (static capability registry linked at validation time), `runtara-ai`.
- Key integration point: `compile::compile_workflow` — the server calls it after a `ChildWorkflowInput` list is resolved; the result is an artifact path plus metadata the dispatcher registers for execution.
- This crate is a host-side build tool (runs on native host). The *output* runs as a WASM guest inside wasmtime on the workflow-instance side.

## License

AGPL-3.0-or-later.
