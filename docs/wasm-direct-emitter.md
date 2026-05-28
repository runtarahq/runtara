# Direct WebAssembly Workflow Emitter Production Migration Plan

## Status

Production migration plan. This document describes the step-by-step path from
the current Rust-codegen workflow compiler to a production direct WebAssembly
emitter that produces small workflow-specific Wasm components and reuses
prebuilt shared components for workflow stdlib behavior and agents.

The current proof of concept lives in:

- `crates/runtara-workflows/src/direct_wasm_poc.rs`
- `crates/runtara-workflows/src/bin/direct_wasm_poc.rs`

That PoC emits a valid core Wasm module for a tiny control-flow subset. It is
not the production ABI and should be treated only as context for measurement
and learning.

Current implementation progress on `codex/wasm-direct-emitter`:

- `direct_wasm_poc` baseline and comparison CLI exist for measurement only.
- `workflow_features` analyzes parsed `ExecutionGraph` values recursively and
  reports step features, nested graphs, agent ids, connections, child workflow
  references, durability, and routing features.
- `direct_wasm::manifest` builds a deterministic versioned manifest with a
  checksum, sorted steps, sorted edges, nested graph manifests, schemas,
  variables, manifest-wide mapping IDs, manifest-wide condition IDs,
  manifest-wide Filter IDs, manifest-wide Switch IDs, manifest-wide GroupBy
  IDs, and a feature summary.
- `direct_wasm::support` produces deterministic unsupported-feature reports.
  The current production-shaped direct path supports a single entry `Finish`
  step, pure `Conditional` true/false decision trees ending in `Finish` leaves,
  and normal-edge `Filter`/value `Switch`/`GroupBy` chains ending in `Finish`
  leaves, with no breakpoints or other routing. `Finish.inputMapping` forms
  remain broadly supported because mapping semantics are delegated to the shared
  stdlib.
- `direct_wasm::compile::compile_direct_workflow` is an opt-in entry point that
  emits a valid component-format artifact for the currently supported direct
  graph shapes,
  imports the workflow stdlib/runtime interfaces, exports `wasi:cli/run@0.2.3`,
  writes `workflow-logic.wasm`, `manifest.json`, `support-report.json`,
  `wit/world.wit`, and `workflow.wac`, and does not generate a Rust crate. The
  exported run entry now initializes the stdlib with the manifest, loads runtime
  input, builds the mapping source, applies the entry `Finish` mapping by
  manifest mapping ID, and calls `runtara:workflow-runtime/runtime.complete`.
- `runtara-workflow-wit` now defines the first checked-in workflow WIT
  contracts: `runtara:workflow-stdlib/json@0.1.0` for shared JSON semantics and
  `runtara:workflow-runtime/runtime@0.1.0` for SDK/runtime lifecycle calls.
- `runtara-workflow-stdlib::direct_json` now contains the pure Rust
  implementation behind the direct JSON stdlib contract: manifest mapping
  lookup, source-envelope construction, mapping application, template rendering,
  type hints, and Finish `outputs` unwrapping.
- `runtara-workflow-stdlib` now has a `direct-component` feature and
  component metadata for `runtara:workflow-stdlib`. That feature builds a
  `wasm32-wasip2` stdlib component without pulling in SDK/runtime, HTTP, AI,
  or native agent dependencies; default/native builds keep the existing Rust
  crate API and runtime exports.
- `runtara-workflow-runtime` now builds the separate
  `runtara:workflow-runtime` component. It lazily initializes the existing SDK
  from environment variables and exports the first runtime lifecycle surface:
  input loading, completion/failure, custom events, heartbeat, cancellation
  polling, and durable sleep.
- `scripts/build-agent-components.sh` now builds and stages the direct workflow
  stdlib/runtime components beside agent components with sibling metadata, and
  the bundle installer treats `RUNTARA_AGENT_COMPONENTS_DIR` as the shared
  component directory for agents plus direct workflow components.
- `direct_wasm::component` emits component-facing sidecars for static
  composition with separate stdlib and runtime components plus any required
  agents.
- `direct_wasm::compile::compose_direct_workflow` now performs the first
  direct static composition path: it maps the direct `workflow-logic.wasm`
  component plus prebuilt stdlib/runtime components into `wac compose`, writes
  the runtime-facing `workflow.wasm`, and promotes the primary direct compile
  result metadata to the composed artifact.
- `direct_wasm::compile::compile_direct_workflow_composed` now provides the
  first direct compile entry that returns the final static
  `workflow.wasm` artifact shape while retaining `workflow-logic.wasm` for
  debugging and manifest validation.
- `tests/direct_wasm_execute.rs` now provides gated direct execution smoke
  tests. With `RUNTARA_RUN_DIRECT_WASM_E2E=1`, it compiles and statically
  composes the simple `Finish` fixture plus flat and nested `Conditional`
  fixtures plus simple `Filter -> Finish`, value `Switch -> Finish`, and
  `GroupBy -> Finish` fixtures, runs each final
  `workflow.wasm` under
  `wasmtime run --wasi http --wasi inherit-network`, and asserts the fake SDK
  receives the expected mapped completion payloads for the Finish path,
  conditional branches, Filter output, value Switch output, and GroupBy output.
- `tests/direct_wasm_finish_parity.rs` now compares direct `Finish` mapping
  output against the current Rust-generated mapping contract for representative
  fixture shapes: data passthrough, dotted `outputs.*` unwrap, templates,
  composites, variables, step references, type hints, and defaults for missing
  or `null` references.
- `direct_wasm::manifest` now assigns manifest-wide condition IDs for
  `Conditional.condition` expressions, and `runtara-workflow-stdlib::direct_json`
  implements `eval-condition` for the current generated-code condition contract,
  including logical operators, comparisons, equality, string/array operators,
  `LENGTH`, emptiness checks, truthy value expressions, and server-side-only
  operators falling back to `false`.
- `direct_wasm::manifest` now assigns manifest-wide Filter IDs for
  `Filter.config`, and `runtara-workflow-stdlib::direct_json` implements the
  shared `filter` helper for array filtering, non-array inputs, condition
  evaluation against `item`, and step-context insertion.
- `direct_wasm::manifest` now assigns manifest-wide Switch IDs for
  `Switch.config`, and `runtara-workflow-stdlib::direct_json` implements the
  shared `value-switch` helper for non-routing Switch cases, first-match
  selection, default output, output reference resolution, array equality
  shorthand, `BETWEEN`, `RANGE`, and step-context insertion.
- `direct_wasm::manifest` now assigns manifest-wide GroupBy IDs for
  `GroupBy.config`, and `runtara-workflow-stdlib::direct_json` implements the
  shared `group-by` helper for simple keys, nested keys, null keys, non-array
  inputs, expected key initialization, and step-context insertion.
- The direct core emitter now supports pure conditional decision trees:
  each `Conditional` has exactly two labeled `true`/`false` edges to another
  supported direct-control step, and all leaves are `Finish` steps. It calls
  `stdlib.eval-condition`, branches on the returned bool, applies the selected
  `Finish` mapping, and completes through the runtime component. Other routing
  shapes remain rejected by the support gate.
- The direct core emitter now also supports the first normal-edge JSON steps:
  `Filter -> Finish`, value `Switch -> Finish`, and `GroupBy -> Finish`. It
  calls the relevant stdlib helper, uses the returned `steps` context to rebuild
  the mapping source, then applies the terminal `Finish` mapping against
  `steps.<step>.outputs.*`.
- `tests/direct_wasm_condition_parity.rs` now compares direct conditional
  branch selection and selected `Finish` output against the current
  generated-code condition semantics for representative fixtures, including
  boolean equality, `LENGTH`-based numeric comparison, and nested conditionals.
- `tests/direct_wasm_group_by_parity.rs` now compares the direct GroupBy stdlib
  helper against current generated-code GroupBy semantics for simple groups,
  nested key paths, and `expectedKeys`.
- `tests/direct_wasm_filter_parity.rs` now compares the direct Filter stdlib
  helper against current generated-code Filter semantics for simple equality,
  `NOT`, and nested boolean conditions.
- `tests/direct_wasm_switch_value_parity.rs` now compares the direct
  value-switch stdlib helper against current generated-code value Switch
  semantics for first-match behavior, array equality shorthand, default output,
  `BETWEEN`, and `RANGE`.

## Final Goal

The final production result is:

- no per-workflow Rust crate generation;
- no per-workflow `cargo component build`;
- workflow-specific Wasm emitted directly from the typed DSL;
- shared workflow stdlib compiled once and distributed with agent components;
- workflow stdlib and agents statically composed into each final workflow
  artifact;
- final workflow artifacts preserve current runtime behavior, durability,
  checkpointing, debug events, errors, signals, and agent dispatch semantics;
- current Rust-codegen path removed or retained only as an emergency fallback
  after direct mode has full parity.

The primary success metric is production-safe replacement of Rust codegen, not
a broader PoC.

## Current Pipeline

Current workflow compilation is:

```text
DSL JSON
  -> runtara-dsl typed ExecutionGraph
  -> Rust AST/source generation
  -> per-workflow Cargo component crate
  -> cargo component build --target wasm32-wasip2
  -> wac compose with agent components
  -> workflow.wasm
```

Important current properties:

- `compile_workflow` always routes through components mode.
- Generated workflow Rust imports `runtara-workflow-stdlib` as a Rust path
  dependency, so serde, mapping helpers, SDK glue, template rendering, and
  other shared logic are compiled into each workflow.
- Agent components already use a stable byte-buffer WIT ABI:
  `invoke(capability-id, input: list<u8>, connection) -> result<list<u8>, error-info>`.
- The environment runner expects one final `workflow.wasm` file and runs it via
  `wasmtime run --wasi http --wasi inherit-network`.
- Workflow input and output flow through runtara-core persistence via the SDK,
  not stdin/stdout.

## Target Architecture

The target architecture keeps workflow-specific Wasm small and moves reusable
semantics into prebuilt components.

```text
DSL JSON
  -> runtara-dsl typed ExecutionGraph
  -> direct workflow emitter
  -> workflow-logic.wasm/component
       imports runtara:workflow-stdlib/*
       imports runtara:agent-<id>/capabilities
       exports wasi:cli/run
  -> wac compose with:
       workflow-stdlib.wasm
       agent components
  -> workflow.wasm
```

The production target is always one statically composed `workflow.wasm`. Runtime
dynamic linking of workflow stdlib or agent components is intentionally out of
scope. Static composition keeps the current environment runner model and removes
the expensive per-workflow Rust compile without introducing runtime linker
state.

## Design Principles

- Direct-emitted workflow Wasm owns workflow-specific control flow.
- Shared stdlib owns JSON-heavy semantics: mapping, references, templates,
  condition evaluation, envelopes, validation, and runtime lifecycle calls.
- Agents keep the current per-agent WIT `invoke` shape.
- Use `list<u8>` JSON buffers for first production ABI. Handles can be
  optimized later if profiling justifies the complexity.
- Preserve current workflow behavior through differential tests before removing
  the Rust-codegen path.
- Keep direct emitter and current compiler side-by-side until every supported
  step type passes parity tests.

## Component Model Shape

### Workflow Logic Component

The direct emitter should generate a component that:

- exports `wasi:cli/run`;
- imports workflow stdlib interfaces;
- imports each used agent component interface;
- stores workflow-specific metadata in static data/custom sections;
- emits only control-flow and call glue.

The direct emitter should not embed serde, minijinja, SDK HTTP client code, or
large Rust helper implementations in every workflow.

### Workflow Stdlib Component

`runtara-workflow-stdlib` should gain a component build target while preserving
its current Rust crate API for the existing compiler.

Candidate WIT package:

```wit
package runtara:workflow-stdlib@0.1.0;

interface json {
  init-manifest: func(manifest: list<u8>) -> result<_, string>;

  build-source: func(
    data: list<u8>,
    variables: list<u8>,
    steps: list<u8>
  ) -> result<list<u8>, string>;

  apply-mapping: func(
    mapping-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  eval-condition: func(
    condition-id: u32,
    source: list<u8>
  ) -> result<bool, string>;

  process-switch: func(
    switch-id: u32,
    source: list<u8>
  ) -> result<string, string>;

  value-switch: func(
    switch-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  filter: func(
    filter-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  group-by: func(
    group-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;
}

interface runtime {
  load-input: func() -> result<list<u8>, string>;
  complete: func(output: list<u8>) -> result<_, string>;
  fail: func(error: list<u8>) -> result<_, string>;
  custom-event: func(kind: string, payload: list<u8>) -> result<_, string>;
  heartbeat: func() -> result<_, string>;
  is-cancelled: func() -> result<bool, string>;
  durable-sleep: func(ms: u64) -> result<_, string>;
}
```

This can be split into smaller WIT interfaces as it stabilizes. The important
early decision is to keep the data boundary byte-oriented, matching agents.

### Agent Components

Agents should remain as-is for this migration. The direct workflow emitter
should import only agents used by the graph, using the same per-agent interface
names currently generated by components mode:

```wit
import runtara:agent-crypto/capabilities@0.3.0;
import runtara:agent-http/capabilities@0.3.0;
```

## Workflow Manifest

The direct emitter should serialize workflow-specific data into a compact
manifest consumed by the stdlib.

Candidate manifest fields:

```json
{
  "version": 1,
  "workflow_id": "...",
  "template_major": "...",
  "entry_point": "step-id",
  "steps": {
    "step-id": {
      "type": "Agent",
      "name": "Human name",
      "mapping_id": 0,
      "condition_id": null,
      "agent_id": "crypto",
      "capability_id": "hash"
    }
  },
  "mappings": [],
  "conditions": [],
  "switches": [],
  "schemas": [],
  "variables": {},
  "edges": []
}
```

The manifest can start as JSON for simplicity. A binary format or table layout
can come later after profiling.

Two reasonable loading strategies:

1. Embed manifest bytes in the workflow component and pass them once to
   `stdlib.init-manifest`.
2. Include manifest data in a custom section and let the stdlib/runtime read it
   only if the component host exposes such access.

For the static `wac compose` path, option 1 is simpler.

## Direct Emitter Responsibilities

The direct emitter should emit:

- function table or structured code for step execution;
- control-flow branches for normal edges;
- control-flow branches for true/false conditional edges;
- edge-condition routing and priority order;
- loops for `While`, `Split`, and retry/polling shapes when supported;
- calls into stdlib for mapping, conditions, source construction, events, and
  output envelopes;
- calls into agent imports for `Agent` steps;
- calls into runtime imports for lifecycle completion/failure/suspension;
- stable cache keys and scope ids, but not the checkpoint implementation.

The direct emitter should not reimplement:

- JSON path lookup;
- type coercion;
- template rendering;
- condition semantics;
- nested reference resolution;
- SDK HTTP protocol;
- checkpoint/retry storage;
- agent connection resolution.

## Branching vs Condition Evaluation

Branching remains workflow-specific control flow.

```text
condition_result = stdlib.eval-condition(condition_id, source)
if condition_result:
  execute true branch
else:
  execute false branch
```

Condition evaluation belongs in stdlib because it includes reference lookup,
type coercion, logical nesting, string/array operators, null/default handling,
and shared semantics with `Filter`, `Switch`, and edge conditions.

Later optimization can inline simple predicates such as `data.flag == true`.
That should be treated as an optimization, not the baseline design.

## Composition Strategy

### Static Composition

Use `wac compose` as the production composition mechanism:

```text
let stdlib = new runtara:workflow-stdlib { ... };
let agent-crypto = new runtara:agent-crypto { ... };
let wf = new runtara:workflow-logic {
  ...stdlib,
  ...agent-crypto,
  ...
};
export wf...;
```

Compiler changes:

- stage `workflow-stdlib.wasm` beside agent components;
- pass `-d runtara:workflow-stdlib=<path>` to `wac compose`;
- include stdlib WIT deps in generated workflow WIT;
- include stdlib version/checksum in workflow image metadata;
- keep output as one final `workflow.wasm`.

## Step Migration Plan

### Tier 1: Pure JSON and Control Flow

Implement first:

- `Finish`
- `Conditional`
- `Switch`
- `Filter`
- `GroupBy`
- `Log`
- `Error`

Rationale:

- validates graph traversal and control flow;
- exercises mapping and condition semantics;
- does not require agent dispatch or durable checkpointing;
- can be differential-tested against existing fixtures.

Emitter behavior:

- inline control flow;
- call `apply-mapping`;
- call `eval-condition`;
- call `process-switch`, `value-switch`, `filter`, and `group-by`;
- call runtime `custom-event` for debug/log/error events;
- call runtime `complete` or `fail` at terminal points.

### Tier 2: Agent Steps

Implement `Agent` after stdlib source/mapping and runtime lifecycle are stable.

Emitter behavior:

- build source;
- apply input mapping;
- resolve/fetch connection through stdlib/runtime helper;
- call per-agent `capabilities.invoke`;
- convert `error-info` to the same JSON error envelope used today;
- route `onError` edges using existing edge-priority semantics;
- delegate checkpoint/retry/rate-limit behavior to stdlib/runtime.

### Tier 3: Loops and Collections

Implement:

- `Split` sequential mode;
- `While`;
- `Delay` durable mode.

Rationale:

- current Wasm path already favors sequential split behavior;
- loop variables and scope ids are subtle but local enough to test;
- durable sleep and cancellation checks require runtime imports.

Required parity:

- `_loop_indices`;
- `_loop`;
- `_item`;
- `_previousOutputs`;
- scope id generation;
- heartbeat and cancellation checks;
- schema validation for split input/output.

### Tier 4: Embedded Children

Implement `EmbedWorkflow` after checkpoint/cache-prefix behavior is shared.

Options:

1. Inline child graphs into the same directly emitted component.
2. Treat child workflows as separately composed workflow components.

Initial recommendation: inline preloaded child graphs, matching current compile
behavior, then revisit separately linked child workflows later.

Required parity:

- child input validation;
- child variable isolation;
- parent scope id;
- child cache key prefix;
- structured child error wrapping;
- child workflow terminal error propagation.

### Tier 5: Long-Lived and AI Steps

Implement last:

- `WaitForSignal`
- `AiAgent`

Reasons:

- long-lived wait/poll/suspend semantics;
- generated signal ids;
- `on_wait` subgraphs;
- timeout and cancellation behavior;
- LLM provider state;
- tool dispatch loops;
- MCP synthetic tools;
- memory providers;
- nested `WaitForSignal` and `EmbedWorkflow` tool targets.

Before migrating `AiAgent`, keep `runtara-ai` as a separate statically composed
Wasm component, possibly shared by `ai-tools`, instead of linking it into the
workflow stdlib component.

## Durability and Checkpoints

Durability should not be compiled as raw Wasm logic. The direct emitter should
generate stable ids and control-flow boundaries, while stdlib/runtime owns:

- checkpoint lookup/write;
- retry loops;
- retry category handling;
- rate-limit budget accounting;
- durable sleep;
- cancellation checks;
- resume from checkpoint;
- failure classification.

Candidate runtime ABI additions:

```wit
interface checkpoint {
  run-once: func(key: string, input: list<u8>) -> result<list<u8>, string>;
  begin-step: func(key: string, metadata: list<u8>) -> result<option<list<u8>>, string>;
  finish-step: func(key: string, output: list<u8>) -> result<_, string>;
  fail-step: func(key: string, error: list<u8>) -> result<_, string>;
}
```

The exact API needs a focused spike because `#[resilient]` currently hides a
lot of behavior behind Rust macros and global SDK state.

## Error Routing

Direct emitter owns edge routing:

- normal edges;
- `onError` edges;
- conditional edge priority;
- default edge fallback.

Stdlib owns:

- error envelope construction;
- error category extraction;
- edge condition evaluation against `__error`;
- error payload truncation for events.

Parity target: current behavior in `program::emit_execute_workflow`, especially
the routing logic that injects `__error` into the source context.

## Testing Strategy

### Unit Tests

Add tests for:

- stdlib mapping parity;
- stdlib condition parity;
- source/path lookup parity;
- envelope generation;
- direct emitter import/export structure;
- manifest encoding/decoding;
- WIT ABI compatibility.

### Differential Tests

For each fixture:

1. Compile with current Rust/component path.
2. Compile with direct emitter path.
3. Run both.
4. Compare:
   - final status;
   - final output;
   - structured error;
   - emitted debug/log events where enabled;
   - checkpoint behavior for durable steps.

Existing fixture coverage under `crates/runtara-workflows/tests/fixtures` should
be reused and expanded.

### Performance Tests

Track:

- direct emit time;
- current Rust artifact codegen time;
- full current compile time;
- direct composed artifact size;
- current composed artifact size;
- cold and warm execution time;
- memory peak.

The existing PoC CLI already reports direct emit and current codegen sizes; keep
that shape and add production metrics as the direct component path matures.

## Migration and Rollout

1. Keep existing `compile_workflow` as default.
2. Add an explicit compile mode or env flag:
   `RUNTARA_WORKFLOW_COMPILE_MODE=direct-wasm`.
3. Enable direct mode only for fixtures and internal testing first.
4. Gate direct mode by feature support. If a graph includes unsupported steps,
   fall back to current compiler or fail with a precise unsupported-step report.
5. Add per-workflow metadata:
   - source checksum;
   - template major;
   - direct emitter version;
   - stdlib ABI version;
   - stdlib checksum;
   - agent component checksums.
6. Run A/B in CI for supported fixtures.
7. Enable for pure JSON/control workflows.
8. Expand to agent workflows.
9. Retire Rust codegen only after complete step parity and production soak.

## Cache Invalidation

Workflow artifact cache keys must include:

- workflow source checksum;
- direct emitter version;
- stdlib WIT ABI version;
- stdlib component checksum;
- agent component checksums;
- DSL schema/template major;
- relevant compile flags.

Any stdlib ABI-breaking change should invalidate direct-emitted workflows.

Internal stdlib implementation changes can avoid invalidation only if the WIT
ABI and behavior are explicitly compatible. In practice, use checksum-based
invalidation until the ABI is stable.

## Packaging

Extend component bundle generation to include:

- `workflow_stdlib.wasm`;
- `workflow_stdlib.meta.json` or equivalent;
- workflow stdlib WIT package;
- existing agent `.wasm` files;
- existing agent `.meta.json` files.

Possible env names:

- `RUNTARA_WORKFLOW_STDLIB_COMPONENT`
- or reuse `RUNTARA_AGENT_COMPONENTS_DIR` as a general component bundle dir.

Recommendation: rename conceptually to a component bundle dir over time, but
keep compatibility with the current agent env var.

## Risks

### Component ABI Emission

Direct core Wasm is simple. Direct component-model emission with correct
canonical ABI for strings, lists, records, imports, and exports is more complex.

Mitigation:

- isolate component encoding into a small `component_abi` module;
- start with one imported stdlib function and `wasi:cli/run`;
- validate generated components with Wasmtime and `wac compose`;
- keep the PoC core-Wasm emitter as a learning artifact, not the production
  foundation.

### Semantic Drift

Generated Rust currently duplicates behavior that also exists in stdlib. One
known issue is nested reference handling around `item` paths. Moving semantics
behind stdlib must include parity tests before migration.

### Durability Regression

Checkpoint/retry behavior is central to workflow correctness. Do not migrate
durable agent/embed/sleep behavior until the runtime ABI is explicit and tested
against crash/resume scenarios.

### Artifact Size

Static composition with stdlib may still duplicate stdlib bytes per workflow.
That is an accepted tradeoff. The migration optimizes production compile
latency and operational simplicity first, not cross-workflow binary
deduplication.

### Tooling

The direct production path depends on static composition. `wac compose` is the
initial production tool for that composition. Replacing `wac` with an in-process
static composer can be considered later, but runtime dynamic linking is not a
goal.

## Open Questions

- Should stdlib expose pure JSON functions only, or also runtime SDK calls? - we probably want to have a separate component for STDLIB and a separate one for SDK
- Should runtime lifecycle be a stdlib component, host imports, or both? - clarification needed, ask me.
- Should manifests be JSON, CBOR, or static Wasm data tables? - like now, JSON
- Should direct-emitted workflows use a state-machine interpreter model or
  generated structured control flow? - generated control flow as now
- Should child workflows be inlined into the workflow-logic component or
  statically composed as separate workflow components? - separate components. this means we need to store separate the whole assembled bundle and "naked" workflow. No inlining, unless it substantially complicates the process

## Step-by-Step Production Migration Plan

Each phase has explicit implementation steps and a checkpoint. Do not advance
to the next phase until the checkpoint passes in CI and locally.

### Phase 0: Baseline and Safety Harness

Goal: establish measurable parity targets before changing compiler behavior.

Implementation steps:

1. Inventory all workflow fixtures under `crates/runtara-workflows/tests/fixtures`
   by step type, durability mode, agent use, child workflow use, and signal use.
2. Add a fixture capability matrix that marks each fixture as:
   - pure JSON/control;
   - agent;
   - durable;
   - child workflow;
   - wait/signal;
   - AI agent.
3. Add a reusable differential test harness that can:
   - compile a fixture with the current Rust/component path;
   - compile the same fixture with the direct path;
   - run both artifacts;
   - compare status, output, structured errors, and event records.
4. Add baseline measurements for current compiler:
   - Rust artifact codegen time;
   - `cargo component build` time;
   - `wac compose` time;
   - final artifact size;
   - cold/warm execution time.
5. Keep `compile_workflow` unchanged.

Checkpoint 0:

- Fixture matrix exists.
- Differential test harness runs at least one current Rust/component artifact.
- Baseline metrics are emitted in CI logs or test output.
- No production compile behavior changes.

Rollback:

- Remove test-only harness and metrics if it destabilizes CI.

### Phase 1: Extract Stdlib Semantics Behind Rust APIs

Goal: move duplicated generated helper semantics into reusable Rust stdlib
functions before introducing WIT.

Implementation steps:

1. Move source construction and JSON path lookup out of generated workflow code:
   - `__build_step_source`;
   - `__lookup_source_pointer`;
   - `__lookup_source_path`;
   - `__path_to_json_pointer_runtime`.
2. Move mapping evaluation behind a runtime function:
   - references;
   - immediate values;
   - composite values;
   - dotted-key insertion;
   - type hints;
   - template values.
3. Move condition evaluation behind a runtime function:
   - logical ops;
   - comparison ops;
   - string ops;
   - array ops;
   - empty/defined/length semantics;
   - server-only operator handling.
4. Move shared envelope/event helpers:
   - step output envelope;
   - embed output envelope;
   - agent error output;
   - debug event payload construction;
   - truncation.
5. Move nested reference resolution into one canonical implementation and fix
   semantic drift around qualified `item` references.
6. Update the existing Rust codegen to call these Rust stdlib APIs instead of
   emitting duplicated helper bodies.

Checkpoint 1:

- Existing Rust/component compiler still passes current tests.
- Generated `src/lib.rs` size drops for representative fixtures.
- Unit parity tests prove old generated helper behavior matches new stdlib
  behavior for mapping, conditions, source lookup, templates, and envelopes.
- Known `item` reference drift is fixed or explicitly documented with tests.

Rollback:

- Keep old generated helper emitters behind a temporary feature flag until
  stdlib parity tests are stable.

### Phase 2: Define Production Workflow Stdlib WIT

Goal: commit to the first production WIT ABI for shared workflow stdlib.

Current status:

- The initial WIT contracts are checked in under `crates/runtara-workflow-wit`.
- Stdlib and runtime are separate WIT packages:
  - `runtara:workflow-stdlib/json@0.1.0`;
  - `runtara:workflow-runtime/runtime@0.1.0`.
- Unit tests parse both packages with `wit-parser` and assert the expected
  exported worlds and functions.
- `runtara-workflow-stdlib` includes generated WIT bindings and a
  `direct-component` implementation for the JSON stdlib surface. Implemented
  functions are `init-manifest`, `build-source`, `apply-mapping`,
  `eval-condition`, `value-switch`, `filter`, and `group-by`; `process-switch`
  remains reserved for routing Switch dispatch.
- `runtara-workflow-runtime` includes generated WIT bindings and implements
  the runtime lifecycle surface against `runtara-sdk`.
- Remaining work: add host-side bindings smoke tests that instantiate and call
  the stdlib/runtime components.

Implementation steps:

1. Add canonical workflow WIT packages.
2. Define byte-buffer JSON interfaces for:
   - manifest initialization;
   - source construction;
   - mapping evaluation;
   - condition evaluation;
   - switch routing;
   - filter/group-by;
   - envelopes;
   - runtime lifecycle.
3. Keep WIT records minimal. Prefer `list<u8>` JSON buffers and strings over
   deep WIT records for the first version.
4. Add an ABI version constant and semver policy:
   - patch: behavior-compatible implementation changes;
   - minor: additive WIT changes;
   - major: breaking WIT or manifest changes.
5. Add WIT validation tests and generated bindings smoke tests.

Checkpoint 2:

- WIT package is checked in.
- ABI versioning policy is documented.
- Wasmtime can instantiate a minimal stdlib component and call one pure helper.
- Current Rust-codegen path still builds without depending on the WIT path.

Rollback:

- WIT is additive only at this phase; no production caller depends on it yet.

### Phase 3: Build and Bundle Workflow Stdlib Component

Goal: compile workflow stdlib once and distribute it beside agent components.

Current status:

- `runtara-workflow-stdlib` can be built as a static component with:
  `cargo component build --target wasm32-wasip2 -p runtara-workflow-stdlib --no-default-features --features direct-component`.
- `runtara-workflow-runtime` can be built as a static component with:
  `cargo component build --target wasm32-wasip2 -p runtara-workflow-runtime --no-default-features --features wasi`.
- The `direct-component` feature keeps the stdlib component byte surface focused
  on shared JSON semantics while default/native/wasi/wasm-js builds continue to
  expose the existing Rust SDK runtime API.
- The generated components export `runtara:workflow-stdlib/json@0.1.0` and
  `runtara:workflow-runtime/runtime@0.1.0`, so both shared components are ready
  for the first direct workflow composition smoke test.
- Component build scripts now stage:
  - `runtara_workflow_stdlib.wasm`;
  - `runtara_workflow_stdlib.meta.json`;
  - `runtara_workflow_runtime.wasm`;
  - `runtara_workflow_runtime.meta.json`.

Implementation steps:

1. Add component build metadata to `runtara-workflow-stdlib` or create a sibling
   component crate if the existing crate shape makes that cleaner.
2. Build `workflow_stdlib.wasm` for `wasm32-wasip2`.
3. Emit `workflow_stdlib.meta.json` with:
   - package name;
   - WIT version;
   - crate version;
   - checksum;
   - build timestamp or reproducible build id;
   - exported interface list.
4. Update component build scripts to package:
   - workflow stdlib component;
   - workflow stdlib WIT;
   - existing agent components;
   - existing agent metadata.
5. Add loader/staging utilities in `runtara-workflows` analogous to agent CAS
   staging.

Checkpoint 3:

- `scripts/build-agent-components.sh` or successor bundle script produces
  stdlib and agents in one component bundle.
- A smoke test composes a trivial workflow component with `workflow_stdlib.wasm`.
- Bundle metadata includes stdlib checksum.

Rollback:

- Continue shipping agent-only bundles; direct compiler remains disabled.

### Phase 4: Direct Component ABI Foundation

Goal: emit a real component-model workflow, not core-Wasm PoC output.

Current status:

- Component-format artifact emission has started. The opt-in direct path now
  emits a valid component with a canonical `wasi:cli/run@0.2.3` export and
  stdlib/runtime component imports.
- The current finish-only direct component proves the direct compile entry,
  sidecars, support-gating, artifact validation, canonical run export, and
  "no generated Rust crate" behavior. The `run` dispatcher calls
  `stdlib.init-manifest`, `runtime.load-input`, `stdlib.build-source`,
  `stdlib.apply-mapping`, and `runtime.complete`.
- `direct_wasm::component` emits `wit/world.wit` and `workflow.wac` sidecars
  that import `runtara:workflow-stdlib/json@0.1.0`,
  `runtara:workflow-runtime/runtime@0.1.0`, export `wasi:cli/run@0.2.3`,
  and statically compose stdlib, runtime, workflow logic, and required agents.
- The current run entry delegates `Finish.inputMapping` semantics to the shared
  stdlib. The pure stdlib implementation now honors mapping purpose metadata,
  including the existing Finish-specific top-level `outputs` unwrap.
- The manifest now assigns deterministic manifest-wide mapping IDs, and run
  lowering calls `stdlib.apply-mapping(mapping-id, source)` without relying on
  implicit step ordering.

Implementation steps:

1. Add a `direct_component` emitter module separate from the current
   `direct_wasm_poc` module.
2. Add a focused component ABI encoder layer for:
   - component header/sections;
   - imports;
   - exports;
   - canonical lifting/lowering;
   - `list<u8>` parameters/results;
   - strings;
   - `result<T, string>`.
3. Emit a minimal workflow component that:
   - imports `runtara:workflow-stdlib/json.eval-condition`;
   - exports `wasi:cli/run`;
   - calls stdlib with a static manifest;
   - calls runtime `complete`.
4. Compose the direct component with `workflow_stdlib.wasm` using `wac`.
5. Run the composed artifact through the current environment runner shape:
   one `workflow.wasm`, `wasmtime run`, SDK env vars.

Checkpoint 4:

- Generated direct component validates with Wasmtime.
- `wac compose` succeeds.
- The final artifact runs under `wasmtime run`.
- No Rust crate is generated for the workflow.
- Current Rust compiler remains default.

Rollback:

- Delete generated direct artifact; no production path depends on it.

### Phase 5: Direct Manifest and Graph Lowering

Goal: lower complete workflow graph metadata into a manifest and generate a
step dispatcher skeleton.

Current status:

- `DirectWorkflowManifest` exists and is covered by deterministic checksum
  tests across parseable workflow fixtures.
- Unsupported reports exist and name exact step ids, step types, feature keys,
  and actionable reasons.
- Single-entry `Finish` graphs can be compiled through the opt-in direct entry
  point into a component-format artifact and sidecar files without generating
  `Cargo.toml`, `src/lib.rs`, or any per-workflow Rust crate.
- The current run dispatcher lowers the entry `Finish` path through
  `runtime.load-input`, `stdlib.build-source`, `stdlib.apply-mapping`, and
  `runtime.complete`, then propagates the `result<_, string>` tag back through
  `wasi:cli/run`.
- The WIT component wrapper around the direct JSON stdlib helpers now builds as
  a standalone `workflow_stdlib.wasm` component under the `direct-component`
  feature.
- The runtime WIT component wrapper now builds as a standalone
  `workflow_runtime.wasm` component under the `wasi` feature and delegates SDK
  lifecycle calls to `runtara-sdk`.
- The direct composition helper can compose supported direct workflow components
  with the prebuilt shared stdlib/runtime components through `wac compose`.
- The composed artifact is now represented in the direct compile result:
  logic-only compilation keeps `wasm_path == workflow_logic_wasm_path`, while
  composition updates `wasm_path` to the final `workflow.wasm` and records
  composed size/checksum metadata.
- Gated direct execution tests now run composed artifacts through the current
  environment runner shape and verify the SDK completion payload.
- Finish mapping parity fixtures now compare direct stdlib output against the
  current Rust-generated mapping contract and cover the default-on-null
  behavior that direct stdlib must preserve.
- Conditional condition IDs are now in the manifest and the direct stdlib
  component can evaluate those conditions through the checked WIT surface.
- Filter config IDs are now in the manifest and the direct stdlib component can
  evaluate Filter configs through the checked WIT surface; parity fixtures cover
  simple equality, `NOT`, nested boolean behavior, and non-array input handling.
- Value Switch config IDs are now in the manifest and the direct stdlib
  component can evaluate non-routing Switch configs through the checked
  `value-switch` WIT surface; parity fixtures cover first-match behavior, array
  equality shorthand, default output, `BETWEEN`, and `RANGE`.
- GroupBy config IDs are now in the manifest and the direct stdlib component
  can evaluate GroupBy configs through the checked WIT surface; parity fixtures
  cover simple, nested-key, expected-key, null-key, and non-array behavior. The
  direct core now consumes helper-updated `steps` contexts for `Filter`,
  value `Switch`, and `GroupBy` normal-edge workflows before rebuilding the
  source and reaching `Finish`.
- The first direct Wasm branching path is implemented for
  `Conditional -> true/false Finish`, and the run-plan lowering now recurses
  through nested pure `Conditional` trees until each branch reaches a `Finish`
  leaf, with support gating kept narrow.
- Gated execution coverage now runs composed conditional artifacts for flat and
  nested branch inputs and verifies the selected Finish output.
- Direct conditional branch parity fixtures now compare direct stdlib
  evaluation and branch output against current generated-code condition
  semantics for simple equality, `LENGTH` comparisons, and nested branch paths.
- Remaining work: broaden graph lowering across more pure JSON/control
  workflows by lowering switch routing, log/error behavior, and edge-condition
  priority handling.

Implementation steps:

1. Define `DirectWorkflowManifest` Rust structs in `runtara-workflows`.
2. Serialize:
   - steps;
   - entry point;
   - normal edges;
   - labeled edges;
   - edge conditions and priorities;
   - mappings;
   - conditions;
   - schemas;
   - variables;
   - agent requirements.
3. Add manifest checksum and schema version.
4. Generate a direct step dispatcher that can:
   - jump to entry point;
   - follow normal edges;
   - terminate through `complete` or `fail`;
   - return unsupported-step errors with exact step ids.
5. Add graph validation that rejects unsupported constructs before emission.

Checkpoint 5:

- Direct compiler emits manifest for every fixture.
- Unsupported fixture reports are deterministic and actionable.
- Pure finish-only workflows and pure conditional `Finish` trees run through the
  direct component path.
- Manifest round-trip tests pass.

Rollback:

- Direct mode remains opt-in and can reject all unsupported graphs.

### Phase 6: Pure JSON and Control-Flow Step Parity

Goal: support non-agent, non-durable workflows end to end.

Current progress:

- `Finish` is implemented for direct execution through the shared mapping
  stdlib and runtime completion surface.
- `Conditional` lowering now supports pure true/false decision trees that end
  in `Finish` leaves. It evaluates each condition through `stdlib.eval-condition`
  and emits nested Wasm `if` control flow in the workflow-specific module.
- `Filter -> Finish`, value `Switch -> Finish`, and `GroupBy -> Finish`
  normal-edge lowering now run end to end. The shared direct stdlib returns an
  updated `steps` context from the step helper, and the direct core rebuilds the
  source before applying the final `Finish` mapping.
- Remaining parity work in this phase starts with switch routing, then broadens
  to `Log`, `Error`, edge conditions, and debug event behavior.

Implementation steps:

1. Implement `Finish`:
   - call `apply-mapping`;
   - call output envelope helper if needed;
   - call runtime `complete`.
2. Implement `Conditional`:
   - call `eval-condition`;
   - branch to true/false targets;
   - preserve debug event behavior.
3. Implement `Switch`:
   - value switch;
   - routing switch;
   - default behavior;
   - output processing parity.
4. Implement `Filter` and `GroupBy` via stdlib helpers.
5. Implement `Log` and `Error`:
   - runtime `custom-event`;
   - structured failure payload;
   - terminal behavior.
6. Implement edge-condition routing:
   - priority order;
   - default fallback;
   - `__error` source injection for error edges where applicable.

Checkpoint 6:

- Differential tests pass for all pure JSON/control fixtures.
- Direct artifacts run through the current environment runner path.
- Debug/log/error event parity is verified for representative fixtures.
- Compile latency shows no Rust build step.

Rollback:

- Feature gate direct mode to pure workflows only.
- Fall back to Rust compiler for any unsupported graph.

### Phase 7: Agent Step Parity

Goal: support workflows that invoke agents through existing per-agent WIT
interfaces.

Implementation steps:

1. Collect used agents from the graph using existing canonicalization rules.
2. Emit per-agent imports in workflow WIT/component metadata.
3. Extend `wac` generation to instantiate/spread required agents and stdlib.
4. Implement `Agent` lowering:
   - source construction;
   - input mapping;
   - agent input validation;
   - connection resolution/envelope;
   - `capabilities.invoke`;
   - success output envelope;
   - `error-info` to JSON error envelope.
5. Implement `onError` routing for agent failures.
6. Preserve current retry policy shape by delegating durable retry behavior to
   stdlib/runtime. Non-durable calls can be supported first if needed.

Checkpoint 7:

- Differential tests pass for representative pure and agent fixtures.
- Missing agent component errors identify the agent id and expected path.
- Agent error envelopes match current behavior.
- Connection-using agent fixtures pass in an integration environment.

Rollback:

- Direct mode rejects agent workflows unless `direct-agent` support is enabled.

### Phase 8: Runtime Lifecycle and Durability ABI

Goal: replace macro-hidden `#[resilient]` behavior with explicit runtime ABI
without changing workflow semantics.

Implementation steps:

1. Specify checkpoint/runtime WIT:
   - begin step;
   - finish step;
   - fail step;
   - retry decision;
   - durable sleep;
   - heartbeat;
   - cancellation check;
   - resume from checkpoint.
2. Implement stdlib/runtime functions using the existing SDK behavior.
3. Generate stable cache keys matching current behavior:
   - workflow id;
   - step id;
   - loop indices;
   - child cache prefixes;
   - retry/rate-limit scope.
4. Migrate durable `Agent`.
5. Migrate `Delay`.
6. Add crash/resume tests:
   - resume after checkpoint;
   - retry transient failure;
   - no retry permanent failure;
   - rate-limit budget exhaustion;
   - cancellation during long-running workflow.

Checkpoint 8:

- Durable agent and delay fixtures pass differential tests.
- Crash/resume tests pass on direct path.
- Current Rust path and direct path produce same persisted instance status.
- Runtime ABI has versioned tests.

Rollback:

- Direct mode remains limited to non-durable workflows until this checkpoint
  passes.

### Phase 9: Split and While

Goal: support loop and collection control flow.

Implementation steps:

1. Implement sequential `Split` first.
2. Preserve split behavior:
   - null and non-array handling;
   - item variable injection;
   - `_loop_indices`;
   - `dontStopOnFailed`;
   - input/output schema validation;
   - output collection shape.
3. Implement `While`:
   - max iterations;
   - condition evaluation before each iteration;
   - `_previousOutputs`;
   - heartbeat/cancellation checks;
   - onError routing.
4. Defer parallel split until runtime support and test coverage are explicit.

Checkpoint 9:

- Existing split and while fixtures pass differential tests.
- Sequential direct split behavior matches current Wasm behavior.
- Loop-scoped references match current generated Rust semantics.

Rollback:

- Direct mode rejects split/while workflows if loop feature flag is disabled.

### Phase 10: EmbedWorkflow

Goal: support embedded child workflows with correct isolation and durability.

Implementation steps:

1. Inline preloaded child graphs into the direct component, matching current
   compiler behavior.
2. Generate separate child graph functions or state-machine regions.
3. Preserve:
   - child input validation;
   - child default variables;
   - parent scope id;
   - child scope id;
   - child cache key prefix;
   - child error wrapping;
   - terminal Error propagation;
   - durable checkpoint boundaries.
4. Add nested child workflow differential tests.

Checkpoint 10:

- Child workflow fixtures pass.
- Nested child workflows pass.
- Child failure and parent `onError` behavior match current path.

Rollback:

- Direct mode rejects `EmbedWorkflow` graphs until enabled.

Long-term choice:

- Prefer inlining child graphs into the same workflow-logic component unless
  static composition of child workflow components gives a clear build or reuse
  benefit.
- Do not introduce runtime child-workflow dynamic linking.

### Phase 11: WaitForSignal

Goal: support long-lived external wait behavior.

Implementation steps:

1. Specify signal runtime ABI:
   - generate signal id;
   - emit waiting event/action metadata;
   - execute `on_wait` subgraph;
   - poll or suspend;
   - timeout;
   - resume with signal payload;
   - cancellation.
2. Implement `WaitForSignal` lowering.
3. Add tests for:
   - normal signal resume;
   - timeout;
   - cancellation;
   - `on_wait` failure;
   - action metadata and response schema.

Checkpoint 11:

- Wait-for-signal fixtures pass.
- External signal integration test passes.
- Suspended/resumed instance state matches current behavior.

Rollback:

- Direct mode rejects wait/signal workflows until enabled.

### Phase 12: AiAgent

Goal: support AI agent workflows without linking provider logic into every
workflow.

Implementation steps:

1. Move provider calls and AI tool execution behind WIT agent/component
   boundaries, preferably through `ai-tools` or equivalent.
2. Preserve:
   - memory provider load/save;
   - tool loops;
   - MCP synthetic tools;
   - structured output schema injection;
   - compaction;
   - iteration limits;
   - wait/signal and embed workflow tools.
3. Implement direct lowering for the orchestration loop only.
4. Add end-to-end AI fixtures with deterministic/mock providers.

Checkpoint 12:

- AI workflows pass deterministic integration tests.
- Provider-specific logic is not linked into workflow-specific Wasm.
- Tool-call event and error behavior matches current path.

Rollback:

- Direct mode rejects `AiAgent` workflows until enabled.

### Phase 13: CI A/B and Production Shadowing

Goal: prove parity before user-visible rollout.

Implementation steps:

1. Add CI jobs that compile supported fixtures both ways.
2. Add CI jobs that execute both artifacts and diff results.
3. Add optional server-side shadow compilation:
   - production still uses Rust artifact;
   - direct artifact is compiled and stored for comparison only;
   - failures are metrics, not user-visible errors.
4. Add metrics:
   - direct compile success/failure by step type;
   - unsupported-step counts;
   - compile latency;
   - artifact size;
   - execution parity failures;
   - stdlib/agent checksum mismatch.

Checkpoint 13:

- Direct compile succeeds for all enabled feature classes in CI.
- Shadow compilation runs without blocking production deploys.
- No unexplained parity failures for the enabled subset over the agreed soak
  window.

Rollback:

- Disable shadow direct compilation via env/config.

### Phase 14: Controlled Production Enablement

Goal: use direct artifacts for real workflows under controlled gates.

Implementation steps:

1. Add compile mode selection:
   - global config;
   - tenant allowlist;
   - workflow allowlist;
   - automatic fallback policy.
2. Start with pure JSON/control workflows.
3. Enable agent workflows after Phase 7/8 parity.
4. Enable loop/child/signal/AI workflows only after their checkpoints.
5. Keep Rust fallback artifact available during rollout.
6. Add operator-visible diagnostics:
   - why direct mode was selected;
   - why fallback happened;
   - direct compiler version;
   - stdlib checksum;
   - unsupported features.

Checkpoint 14:

- Direct mode handles selected production workflows.
- Fallback is automatic and observable.
- No production incident tied to direct artifacts during soak.
- Compile latency reduction is visible in production metrics.

Rollback:

- Flip compile mode back to Rust globally.
- Existing Rust compiler remains available.
- Previously compiled Rust artifacts remain runnable.

### Phase 15: Default Direct Mode

Goal: make direct emitter the default compiler.

Implementation steps:

1. Switch default compile mode to direct for all fully supported workflows.
2. Keep Rust fallback for one release cycle or agreed deprecation window.
3. Require explicit override to use Rust compiler.
4. Monitor:
   - compile failures;
   - runtime failures;
   - fallback rates;
   - direct/Rust parity canaries;
   - stdlib version mismatches.

Checkpoint 15:

- Direct mode is default.
- Fallback rate is below agreed threshold.
- All supported workflow classes pass production and CI checks.
- Release notes document behavior and rollback.

Rollback:

- Restore Rust compiler as default via config.

### Phase 16: Rust Codegen Retirement

Goal: remove the expensive per-workflow Rust compile path from the production
critical path.

Implementation steps:

1. Remove automatic Rust fallback after the deprecation window.
2. Keep Rust codegen only as:
   - a dev/debug tool; or
   - a separate emergency branch; or
   - remove entirely if no longer needed.
3. Remove per-workflow Cargo crate materialization from production compile.
4. Remove vendored workflow build dependency requirements that are only needed
   for per-workflow Rust compilation.
5. Update docs, install scripts, and operational runbooks.

Checkpoint 16:

- Production no longer invokes `cargo component build` for workflows.
- Direct emitter handles all supported workflow DSL features.
- Operational rollback plan no longer depends on compiling Rust on the fly.
- Documentation is updated.

Rollback:

- Before deleting Rust codegen entirely, tag a release that can be redeployed
  if direct mode has an unexpected production issue.

## Production Checkpoint Summary

| Checkpoint | Required result |
| --- | --- |
| 0 | Baseline metrics and differential harness exist. |
| 1 | Shared Rust stdlib helper parity proven. |
| 2 | Workflow stdlib WIT defined and versioned. |
| 3 | Workflow stdlib component builds and bundles. |
| 4 | Direct component validates, composes, and runs. |
| 5 | Manifest and graph lowering are deterministic. |
| 6 | Pure JSON/control workflows pass parity. |
| 7 | Agent workflows pass parity. |
| 8 | Durable workflows pass crash/resume parity. |
| 9 | Split and While pass parity. |
| 10 | EmbedWorkflow passes parity. |
| 11 | WaitForSignal passes parity. |
| 12 | AiAgent passes deterministic integration tests. |
| 13 | CI A/B and production shadowing are clean. |
| 14 | Controlled production enablement succeeds. |
| 15 | Direct mode becomes default. |
| 16 | Rust codegen exits the production critical path. |

## Production Definition of Done

The direct emitter can replace Rust codegen in production when:

- every DSL step type supported by production has differential parity;
- durable workflows pass crash/resume tests;
- generated components validate and compose deterministically;
- artifact metadata fully captures stdlib/agent versions and checksums;
- cache invalidation handles stdlib, ABI, emitter, and agent changes;
- environment execution needs no special-case behavior for composed artifacts;
- compile latency no longer includes per-workflow Rust compilation;
- fallback and rollback paths have been exercised;
- production shadowing shows no unexplained parity failures;
- direct mode has completed the agreed production soak window;
- operational docs and install scripts are updated.

## Non-Goals Before Production Cutover

- Runtime dynamic linking of stdlib, agents, or child workflows. Static
  composition is the production architecture.
- Handle-based JSON ABI. Byte-buffer JSON is acceptable until profiling proves
  otherwise.
- Parallel split beyond current Wasm behavior.
- Removing static composition. `wac compose` may eventually be replaced by an
  in-process static composer, but final artifacts should remain self-contained
  `workflow.wasm` files.
