# runtara-workflows

Compiles runtara DSL scenarios into standalone native Linux binaries.

## What it is

A compilation library and CLI that turns a `runtara-dsl` `Scenario` (JSON) into a statically-linked, musl-targeted executable that talks to `runtara-core` over the SDK for durability, checkpointing, and signals. The pipeline is: parse DSL, resolve child-scenario and agent dependencies, generate Rust AST via `codegen`, write a source tree, invoke `rustc`, and optionally produce an OCI image. The crate exposes `compile_scenario`, `translate_scenario`, `validate_workflow`, and the `CompilationInput` / `NativeCompilationResult` types; it has no database dependencies and expects callers to resolve and pass in child scenarios.

## Using it standalone

The `runtara-compile` binary compiles a workflow JSON to a native executable. Requires `rustc` and `musl-tools` on the host.

```bash
cargo install --path crates/runtara-workflows
runtara-compile \
  --workflow workflow.json \
  --tenant acme \
  --scenario order-sync \
  --output ./order-sync
```

Other useful flags: `--validate` (no compilation), `--analyze` (report only), `--emit-source <path>` (dump generated Rust), `--debug`, `--verbose`. Build artifacts live under `$DATA_DIR` (default `.data`).

## Inside Runtara

- Primary consumer: `runtara-server` — `src/compiler/` and the scenarios API (`api/services/compilation.rs`, `api/services/scenarios.rs`) drive compilation on scenario create/update.
- Also consumed by `runtara-connections` for connection-bound scenario compilation paths.
- Upstream deps: `runtara-dsl` (scenario/execution-graph types, re-exported), `runtara-agents` (capability inventory linked at validation time), `runtara-ai`.
- Key integration point: `compile::compile_scenario` — the server calls it after a `ChildScenarioInput` list is resolved; the result is a binary path plus metadata the dispatcher registers for execution.
- Runs native only (not WASM): it shells out to `rustc` with a musl target. The compiled output is what later runs as the workflow process; this crate itself is a host-side build tool.

## License

AGPL-3.0-or-later.
