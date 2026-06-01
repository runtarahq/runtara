# Direct Compilation Architecture

How a workflow DSL graph becomes an executable WebAssembly component. This is the
concise overview; see [`wasm-direct-emitter.md`](./wasm-direct-emitter.md) for the
deep dive and [`wasm-components-migration-plan.md`](./wasm-components-migration-plan.md)
for history.

## One compiler, fully in-process

A workflow graph compiles to a WASM **component** that the runtime executes. There
is a single backend, the **direct emitter** (`compile_workflow_direct`): it emits
the workflow's core WASM module *byte-by-byte* from the graph — no Rust source, no
`rustc`, no `cargo`. It then lifts that module into a component and composes the
final `workflow.wasm` entirely **in-process** using the `wac-graph` crate (the same
library the `wac` CLI is built on). No external toolchain or subprocess is invoked
at compile time.

The earlier generated compiler (Rust codegen + `cargo component build`) and its
direct→generated fallback have been removed: every valid graph compiles directly,
and graphs the emitter cannot lower are rejected at validation rather than routed
elsewhere.

## The pipeline: DSL → manifest → core WASM → component

```
ExecutionGraph (DSL JSON)
   │  analyze + lower            (direct_wasm/manifest.rs, plan.rs, compile/*)
   ▼
DirectWorkflowManifest + DirectRunPlan
   │  emit core module           (compile/core_module.rs — raw Wasm encoder)
   ▼
workflow-logic.wasm  (a core module importing the host + stdlib interfaces,
   │                   lifted to a component via wit-component in-process)
   │  in-process compose         (wac-graph: + shared + per-agent components)
   ▼
workflow.wasm  (a WASI component the runtime instantiates)
```

The emitted module is **thin**: it encodes control flow (the run plan) and calls
out to imported functions for everything data-shaped. It does not embed a JSON
library, mapping evaluator, or HTTP client — those live in shared components.

## Components and dependencies

The composed workflow links three kinds of component:

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
(`direct_wasm/component.rs`). They and the per-agent components are **pre-built**
(by `scripts/build-agent-components.sh`, which compiles + checksums them into
`*.meta.json`) and staged in a components directory — `target/wasm32-wasip2/release/`
in development, the bundle's `agents/` directory in production
(`RUNTARA_AGENT_COMPONENTS_DIR`). The compiler reads these prebuilt `.wasm` files at
compile time; it never builds them.

**Toolchain split.** Workflow compilation at runtime needs *no* external tools — it
links the `wac-graph`, `wit-component`, `wit-parser`, and `wasm-encoder` crates and
runs `wasmtime` to execute. `cargo-component` is used only at *build time* (in CI /
the bundle build) to produce the prebuilt agent and shared components; it is not
shipped. WIT files (`runtara-workflow-wit/`) define every interface contract — host
imports, the stdlib ABI, and the per-agent worlds — so independently-built
components link.

## Inside the direct emitter

- **Manifest** (`manifest.rs`) — flattens the graph into a `DirectGraphManifest`:
  interned static strings, per-step records, child-workflow closures, and the
  imported-agent set merged from any embedded children.
- **Run plan** (`plan.rs`) — a `DirectRunPlan` tree describing execution order.
  Branching steps (Conditional / Switch / conditioned edges) lower to structured
  `if/else` with a single shared continuation at the merge point; unconditional
  fan-out is topologically linearised; `Split`/`While`/`WaitForSignal`/`EmbedWorkflow`
  have dedicated lowerings. Diamonds re-converge once (`find_merge_point_n`,
  `direct_wasm/graph_order.rs`).
- **Per-step lowerers** (`compile/*.rs`) — one module per concern (`agent.rs`,
  `split.rs`, `wait.rs`, `embed_workflow.rs`, `dispatcher.rs`, `mapping.rs`,
  `debug.rs`, …) emitting the actual Wasm instructions, talking to the stdlib via
  the indices wired in `core_imports.rs`.
- **Composition** (`compile.rs::compose_workflow_component_in_process`) — parses the
  emitted `workflow.wac` document and resolves each package to its prebuilt `.wasm`
  via `wac-parser`/`wac-resolver`/`wac-graph`, then encodes the composed component
  in memory.
- **Static data** (`static_data.rs`) — string/JSON constants (the manifest, default
  variables, event kinds) baked into linear memory as data segments addressed by
  `(offset, len)`.
- **Support gate** (`support.rs`) — `analyze_direct_wasm_support` decides whether a
  graph is fully lowerable. Unsupported shapes are surfaced as a compile error; a
  valid graph is always lowerable, so the gate effectively never rejects.

## Principles

- **WASM/WASI is the primary target.** Agent and workflow code runs in a component;
  avoid native-only deps and host-thread assumptions.
- **Thin module, fat stdlib.** Emit control flow; delegate data manipulation to the
  shared stdlib component. One JSON/mapping vocabulary, shared by every workflow.
- **No toolchain at runtime.** Compilation is byte emission + in-process linking;
  the server ships no `rustc`, `cargo`, `cargo-component`, or `wac` CLI.
- **Contracts over coupling.** Components interoperate only through WIT interfaces;
  a core module names imports it never defines, and the composition graph resolves
  them.
- **Reject the malformed early.** Ambiguous graphs (e.g. parallel fan-out to
  distinct terminals) are rejected at shared validation, not bent into a lowering.
