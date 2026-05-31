# Direct Compilation Architecture

How a workflow DSL graph becomes an executable WebAssembly component. This is the
concise overview; see [`wasm-direct-emitter.md`](./wasm-direct-emitter.md) for the
deep dive and [`wasm-components-migration-plan.md`](./wasm-components-migration-plan.md)
for history.

## Two compilers, one contract

A workflow graph compiles to a WASM **component** that the runtime executes. There
are two backends producing the same artifact shape:

- **Direct emitter** (`compile_workflow_direct`) — emits the workflow's core WASM
  module *byte-by-byte* from the graph. No Rust source, no `rustc`.
- **Generated compiler** (`compile_workflow`) — emits Rust source for the graph,
  then builds it with `cargo component build`. The legacy path.

`compile_workflow_with_direct_fallback` (server `api/services/compilation.rs`) tries
the direct emitter first and falls back to the generated compiler only if the
**support gate** rejects the graph. The standing goal is **zero fallback**: every
valid graph compiles directly, so the generated compiler can be retired.

Both paths converge on the same final step — `wac compose` — and produce an
interchangeable `workflow.wasm`, which is why the A/B test suite can demand
behavioural parity between them.

## The pipeline: DSL → manifest → core WASM → component

```
ExecutionGraph (DSL JSON)
   │  analyze + lower            (direct_wasm/manifest.rs, plan.rs, compile/*)
   ▼
DirectWorkflowManifest + DirectRunPlan
   │  emit core module           (compile/core_module.rs — raw Wasm encoder)
   ▼
workflow_logic.wasm  (a core module importing the host + stdlib interfaces)
   │  wac compose                (+ shared components + per-agent components)
   ▼
workflow.wasm  (a WASI component the runtime instantiates)
```

The emitted module is **thin**: it encodes control flow (the run plan) and calls
out to imported functions for everything data-shaped. It does not embed a JSON
library, mapping evaluator, or HTTP client — those live in shared components.

## Components and dependencies

The composed workflow links three kinds of component via `wac`:

- **`runtara:workflow-stdlib`** (`runtara_workflow_stdlib.wasm`) — the JSON/runtime
  helper library, built from `runtara-workflow-stdlib` with
  `--no-default-features --features direct-component`. Exports
  `runtara:workflow-stdlib/json` (e.g. `build-source`, `apply-mapping`,
  `eval-condition`, the `split-*` aggregation helpers, `breakpoint-event`). The
  emitted module calls these instead of carrying its own logic.
- **`runtara:workflow-runtime`** (`runtara_workflow_runtime.wasm`) — the host-facing
  runtime surface (load input, checkpoint, custom events, instance id, …),
  implemented over the host imports the runtime provides.
- **Per-agent components** (`runtara_agent_*.wasm`) — one per integration; linked
  only when the graph uses that agent.

The shared components are listed in `DIRECT_SHARED_COMPONENT_REQUIREMENTS`
(`direct_wasm/component.rs`) and are pre-staged in
`target/wasm32-wasip2/release/` (the build script `scripts/build-agent-components.sh`
compiles + checksums them into `*.meta.json`).

**Toolchain.** `cargo-component` (componentizes core modules), `wac` (composition),
`wasmtime` (execution/tests), and `wasmparser`/`wasm-tools` (validation). WIT files
(`runtara-workflow-wit/`) define every interface contract — host imports, the stdlib
ABI, and the per-agent worlds — so independently-built components link.

## Inside the direct emitter

- **Manifest** (`manifest.rs`) — flattens the graph into a `DirectGraphManifest`:
  interned static strings, per-step records, child-workflow closures, and the
  imported-agent set merged from any embedded children.
- **Run plan** (`plan.rs`) — a `DirectRunPlan` tree describing execution order.
  Branching steps (Conditional / Switch / conditioned edges) lower to structured
  `if/else` with a single shared continuation at the merge point; unconditional
  fan-out is topologically linearised; `Split`/`While`/`WaitForSignal`/`EmbedWorkflow`
  have dedicated lowerings. Diamonds re-converge once (`find_merge_point_n`).
- **Per-step lowerers** (`compile/*.rs`) — one module per concern (`agent.rs`,
  `split.rs`, `wait.rs`, `embed_workflow.rs`, `dispatcher.rs`, `mapping.rs`,
  `debug.rs`, …) emitting the actual Wasm instructions, talking to the stdlib via
  the indices wired in `core_imports.rs`.
- **Static data** (`static_data.rs`) — string/JSON constants (the manifest, default
  variables, event kinds) baked into linear memory as data segments addressed by
  `(offset, len)`.
- **Support gate** (`support.rs`) — `analyze_direct_wasm_support` decides whether a
  graph is fully lowerable. On any unsupported feature it returns the reasons and
  the caller falls back. Keeping this gate from ever rejecting is the zero-fallback
  goal.

## Principles

- **WASM/WASI is the primary target.** Agent and workflow code runs in a component;
  avoid native-only deps and host-thread assumptions.
- **Thin module, fat stdlib.** Emit control flow; delegate data manipulation to the
  shared stdlib component. One JSON/mapping vocabulary, shared by every workflow.
- **Determinism & parity.** The direct artifact must match the generated one's
  observable behaviour (output and events), enforced by the A/B suite. Identifier-
  and instance-specific values are normalised, not diverged.
- **Contracts over coupling.** Components interoperate only through WIT interfaces;
  a core module names imports it never defines, and `wac` resolves them.
- **Reject the malformed early.** Ambiguous graphs (e.g. parallel fan-out to
  distinct terminals) are rejected at shared validation, not bent into a lowering —
  so neither compiler has to model them.
