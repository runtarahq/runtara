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
  manifest-wide Split IDs, manifest-wide Filter IDs, manifest-wide Switch IDs,
  manifest-wide GroupBy IDs, manifest-wide Log IDs, manifest-wide Error IDs,
  and a feature summary.
- `direct_wasm::support` produces deterministic unsupported-feature reports.
  The current production-shaped direct path supports a single entry `Finish` or
  `Error` step, pure `Conditional` true/false decision trees ending in
  `Finish`/`Error` leaves,
  normal-edge `Filter`/value `Switch`/`GroupBy`/`Log` chains ending in
  `Finish`/`Error` leaves, and routing `Switch` dispatch trees with one static
  edge per route plus a `default` edge whose leaves can be `Finish` or `Error`.
  Supported normal/`next` edges can now either be a single unconditioned edge or
  a priority-ordered conditional edge set with exactly one unconditioned default
  fallback. Breakpoints remain outside the supported direct-control subset.
  `Finish.inputMapping` forms remain broadly supported because mapping semantics
  are delegated to the shared stdlib.
- `direct_wasm::compile::compile_direct_workflow` is an opt-in entry point that
  emits a valid component-format artifact for the currently supported direct
  graph shapes,
  imports the workflow stdlib/runtime interfaces, exports `wasi:cli/run@0.2.3`,
  writes `workflow-logic.wasm`, `manifest.json`, `support-report.json`,
  `wit/world.wit`, and `workflow.wac`, and does not generate a Rust crate. The
  exported run entry now initializes the stdlib with the manifest, loads runtime
  input, builds the mapping source, dispatches the supported direct run plan,
  and calls `runtara:workflow-runtime/runtime.complete` or
  `runtara:workflow-runtime/runtime.fail` as the selected terminal requires.
- `runtara-workflow-wit` now defines the first checked-in workflow WIT
  contracts: `runtara:workflow-stdlib/json@0.1.0` for shared JSON semantics and
  `runtara:workflow-runtime/runtime@0.1.0` for SDK/runtime lifecycle calls.
- `runtara-workflow-stdlib::direct_json` now contains the pure Rust
  implementation behind the direct JSON stdlib contract: manifest mapping
  lookup, source-envelope construction, mapping application, template rendering,
  type hints, Finish `outputs` unwrapping, and the first Split helpers for
  item normalization, per-iteration variable construction, and result step
  envelopes.
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
  fixtures plus simple `Filter -> Finish`, value `Switch -> Finish`, routing
  `Switch`, `GroupBy -> Finish`, durable/non-durable `Delay -> Finish`,
  durable/non-durable `Agent -> Finish`, cached durable Agent replay,
  `Log -> Finish`, terminal `Error`, and normal-edge condition-priority
  fixtures, runs each final
  `workflow.wasm` under
  `wasmtime run --wasi http --wasi inherit-network`, and asserts the fake SDK
  receives the expected mapped completion payloads for the Finish path,
  conditional branches, Filter output, value Switch output, routing Switch route
  leaves, GroupBy output, Delay output plus durable sleep traffic, Agent output
  plus durable checkpoint traffic, cached Agent checkpoint replay, Log custom
  events, Error custom-event/failure payloads, and condition-priority/default
  routing.
- `tests/direct_wasm_finish_parity.rs` now compares direct `Finish` mapping
  output against the current Rust-generated mapping contract for representative
  fixture shapes: data passthrough, dotted `outputs.*` unwrap, templates,
  composites, variables, step references, type hints, and defaults for missing
  or `null` references.
- `direct_wasm::manifest` now assigns manifest-wide condition IDs for
  `Conditional.condition` expressions and normal-edge conditions, and
  `runtara-workflow-stdlib::direct_json` implements `eval-condition` for the
  current generated-code condition contract, including logical operators,
  comparisons, equality, string/array operators, `LENGTH`, emptiness checks,
  truthy value expressions, and server-side-only operators falling back to
  `false`.
- `direct_wasm::manifest` now assigns manifest-wide Filter IDs for
  `Filter.config`, and `runtara-workflow-stdlib::direct_json` implements the
  shared `filter` helper for array filtering, non-array inputs, condition
  evaluation against `item`, and step-context insertion.
- `direct_wasm::manifest` now assigns manifest-wide Switch IDs for
  `Switch.config`, and `runtara-workflow-stdlib::direct_json` implements the
  shared `process-switch` and `value-switch` helpers for Switch cases:
  first-match selection, default output, selected routing label, route insertion
  into routing Switch step context, output reference resolution, array equality
  shorthand, `BETWEEN`, and `RANGE`.
- `direct_wasm::manifest` now assigns manifest-wide GroupBy IDs for
  `GroupBy.config`, and `runtara-workflow-stdlib::direct_json` implements the
  shared `group-by` helper for simple keys, nested keys, null keys, non-array
  inputs, expected key initialization, and step-context insertion.
- `direct_wasm::manifest` now assigns manifest-wide Log IDs for `Log` steps,
  and `runtara-workflow-stdlib::direct_json` implements the shared `log-event`
  and `log` helpers for generated-code-compatible `workflow_log` payloads,
  context mapping, default level handling, timestamp insertion, and Log
  step-context insertion.
- `direct_wasm::manifest` now assigns manifest-wide Error IDs for `Error`
  steps, and `runtara-workflow-stdlib::direct_json` implements the shared
  `error-event` and `error` helpers for generated-code-compatible
  `workflow_error` payloads, context mapping, default category/severity
  handling, timestamp insertion, and structured workflow-failure payloads.
- The direct core emitter now supports pure conditional decision trees:
  each `Conditional` has exactly two labeled `true`/`false` edges to another
  supported direct-control step, and leaves may be `Finish` or `Error` steps.
  It calls `stdlib.eval-condition`, branches on the returned bool, applies the
  selected `Finish` mapping or emits the selected Error payload, and completes
  or fails through the runtime component. Other routing shapes remain rejected
  by the support gate.
- The direct core emitter now also supports the first JSON transformation and
  dispatch steps: `Filter -> Finish`, value `Switch -> Finish`,
  `GroupBy -> Finish`, `Log -> Finish`, and routing `Switch` trees with static
  route labels. It calls the relevant stdlib helper, uses the returned `steps`
  context to rebuild the mapping source, emits Log payloads through
  `runtime.custom-event`, then applies the selected terminal `Finish` mapping
  against `steps.<step>.outputs.*` and, for routing Switches,
  `steps.<switch>.route`. Terminal `Error` paths emit `workflow_error` through
  `runtime.custom-event`, call `runtime.fail`, and return a failed
  `wasi:cli/run` result instead of completing.
- The direct core emitter now lowers supported normal/`next` edge conditions
  directly into Wasm control flow. Conditional edges are evaluated through
  `stdlib.eval-condition` in descending priority order, and the direct emitter
  follows the explicit default edge when no condition matches.
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
  value-switch and process-switch stdlib helpers against current generated-code
  Switch semantics for first-match behavior, array equality shorthand, default
  output, selected route labels, route context insertion, `BETWEEN`, and
  `RANGE`.
- `tests/direct_wasm_log_parity.rs` now compares the direct Log stdlib helpers
  against current generated-code Log event payload and step-output semantics for
  all log levels and representative context mappings.
- `tests/direct_wasm_error_parity.rs` now compares the direct Error stdlib
  helpers against current generated-code Error event payload and workflow
  failure semantics for explicit and default category/severity cases.
- `track_events` is now wired through the direct compiler. The direct WIT,
  stdlib component, and core emitter support generated-code-compatible
  `step_debug_start`/`step_debug_end` custom events for `Finish`,
  `Conditional`, `Filter`, `Switch`, `GroupBy`, and terminal `Error` steps.
  `Log` remains intentionally limited to its existing `workflow_log` events,
  matching the generated Rust path. Breakpoint pauses remain rejected until the
  runtime/checkpoint ABI can represent durable pause/resume behavior.
- Phase 6 routing scope is now explicit: direct mode supports deterministic
  single-successor normal flow, condition-priority routes with an explicit
  default, and routing Switches with a complete static route/default edge set.
  Parallel fan-out and no-default routing remain rejected until explicit
  direct parallel aggregation and failure semantics are designed.
- Agent manifest preparation is in place. Direct manifests now
  include deterministic `agents` entries plus `agent.inputMapping` mapping
  entries, so the next Agent-lowering slice can refer to stable manifest IDs
  without changing the manifest schema at the same time.
- The workflow-logic component resolver can now include concrete per-agent WIT
  imports (`runtara:agent-<id>/capabilities@0.3.0`) and validates core module
  metadata with those imports present.
- The first Agent execution slice is implemented for Agent normal-flow steps.
  Non-durable Agent support now includes the generated Rust retry defaults and
  retry overrides without checkpoint I/O. The direct core applies the Agent
  input mapping through stdlib, calls the statically imported per-agent
  `capabilities.invoke`, stores the success output through
  `stdlib.agent-output`, rebuilds the source, and continues to the next direct
  run-plan node. The WIT canonical ABI lowers this import indirectly as
  `[pointer, pointer]`, so the direct core now writes the argument area for
  capability id, input bytes, and `option<connection-info>`, and reads the
  Agent-specific result payload offsets for successful output bytes.
- Agent failure handling now converts WIT `error-info` into the same JSON
  envelope used by component codegen, wraps it in the current generated Agent
  step failure string, emits Agent `step_debug_end` failure payloads when
  `track_events` is enabled, calls `runtime.fail`, and returns failed
  `wasi:cli/run`.
- Agent input validation now serializes required capability fields from the
  Agent catalog into the direct manifest. The direct core validates resolved
  Agent inputs before `capabilities.invoke`, emits the same structured
  validation JSON used by generated Rust, emits Agent failure debug payloads
  when `track_events` is enabled, calls `runtime.fail`, and returns failed
  `wasi:cli/run`.
- Non-durable Agent connection ids are now supported. The direct stdlib
  injects the generated Rust-compatible `connection_id` and `_connection`
  fields into Agent JSON input, and the direct core writes the canonical ABI
  `some(connection-info)` record with the connection id, empty integration id,
  `{}` parameters, and no subtype/rate-limit config.
- Agent `onError` routing is now supported for default handlers and
  priority-ordered conditional handlers with at most one default fallback. The
  direct stdlib exposes `error-steps` to insert generated-code-compatible
  `steps.__error`/`steps.error` context, and direct core routes validation and
  capability failures through the handler branch before falling back to
  `runtime.fail` when no condition matches. Agent timeout, compensation, and
  breakpoints remain rejected.
- The first Phase 8 runtime lifecycle ABI slice is in place. The
  `runtara:workflow-runtime` WIT and runtime component now expose checkpoint
  lookup/write, retry-attempt recording, checkpointed durable sleep, and a
  signal-aware checkpoint result wire shape. The runtime also exposes
  `handle-checkpoint-signal`, which acknowledges checkpoint-returned
  `cancel`/`pause`/`shutdown` signals and suspends or cancels without reporting
  workflow completion. Direct Agent lowering now uses these checkpoint,
  retry-attempt, and lifecycle-signal pieces internally. Durable Agent public
  support is enabled for workflows without Agent timeout, compensation, or
  breakpoints; Delay support is now lowered for durable and non-durable normal
  flow, while Delay breakpoints and crash/resume differential tests remain
  pending.
- The shared stdlib now exposes `agent-cache-key`, which centralizes the
  generated Rust-compatible durable Agent key shape using `_workflow_id`,
  `_cache_key_prefix`, and `_loop_indices`. The direct core injects the
  compile workflow id into runtime variables before source construction, so
  root Agent cache keys use the same workflow-id namespace as generated Rust.
  It has an internal no-retry durable Agent checkpoint path that computes this
  key, reads an existing checkpoint before `capabilities.invoke`, and writes a
  checkpoint after successful output. Public support is enabled for the durable
  Agent subset without Agent timeout, compensation, or breakpoints.
- The direct core now also has an internal durable Agent retry loop. It uses
  the generated Rust retry defaults (`maxRetries` override, otherwise 3 or 5
  for rate-limited capabilities), retries only typed WIT Agent errors with
  `error-info.retryable = true`, records retry attempts through
  `runtime.record-retry-attempt` with the raw Agent error JSON payload, lowers
  typed `retryAfterMs` hints to checkpointed `runtime.durable-sleep-checkpoint`
  calls gated by graph `rateLimitBudgetMs`, and checkpoints successful output.
  It also classifies generated-Rust-compatible rate-limit error codes and
  charges rate-limited errors without `retryAfterMs` against the budget using
  the effective base retry delay. Generic retry backoff delay calculation now
  lives in stdlib and direct core sleeps through `runtime.durable-sleep` before
  recording retry attempts. Successful durable Agent checkpoint saves now route
  pending `cancel`/`pause`/`shutdown` signals through the runtime lifecycle
  handler and return before `runtime.complete` when the instance is stopped.
  Public support is now enabled for durable Agent workflows that do not use
  timeout, compensation, or breakpoints. Timeout remains gated because the
  generated Rust Agent path does not currently enforce `AgentStep.timeout`;
  crash/resume differential coverage remains a Phase 8 hardening checkpoint.
- The direct core now has structural and gated host-level replay coverage for
  durable Agent cached checkpoints: the emitted Wasm branch that receives an
  existing checkpoint payload skips both `capabilities.invoke` and
  `runtime.checkpoint`, while the fresh branch still invokes the Agent and
  checkpoints only after success. The direct execution smoke server can preload
  SDK checkpoint responses and verifies cached Agent output flows through
  `Finish` without a fresh Agent invocation. It also verifies a fresh durable
  Agent performs lookup/invoke/save and a non-durable Agent performs no
  checkpoint calls. Full host-level crash/resume differential tests remain
  pending.
- Non-durable Agent retry-loop lowering is now implemented. The direct core
  uses the same default retry counts and base delays as generated Rust, calls
  `stdlib.agent-retry-error-info` and `stdlib.agent-retry-delay-ms` for retry
  classification and backoff calculation, and sleeps through
  `runtime.blocking-sleep`. The emitted non-durable path does not call
  checkpoint lookup/write, durable sleep, retry sleep-key construction, or
  retry-attempt recording, matching `#[resilient(durable = false)]`.
- Delay normal flow is now public in the direct emitter for durable and
  non-durable workflows. The manifest records `Delay` configs, the shared
  stdlib resolves `durationMs` through the same mapping evaluator and emits the
  generated Rust-compatible `steps.<stepId>.duration_ms` shape. Durable Delay
  calls `runtime.durable-sleep-checkpoint(stepId, [], durationMs)`;
  non-durable Delay calls `runtime.blocking-sleep(durationMs)`. Both paths
  rebuild source and continue to the next step. Dynamic durations are covered.
  Delay breakpoints remain gated.

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

  log-event: func(
    log-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  log: func(
    log-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  error-event: func(
    error-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  error: func(
    error-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  group-by: func(
    group-id: u32,
    source: list<u8>
  ) -> result<list<u8>, string>;

  step-debug-start: func(
    step-id: string,
    source: list<u8>
  ) -> result<list<u8>, string>;

  step-debug-end: func(
    step-id: string,
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

Durability should not duplicate SDK persistence logic in raw Wasm. The direct
emitter should generate stable ids and control-flow boundaries, while
stdlib/runtime owns:

- checkpoint lookup/write;
- retry attempt storage;
- retry category handling;
- rate-limit budget accounting;
- durable sleep;
- blocking sleep for non-durable waits;
- cancellation checks;
- resume from checkpoint;
- failure classification.

Current direct runtime ABI additions:

```wit
record signal-info {
  signal-type: string,
  payload: list<u8>,
  checkpoint-id: option<string>,
}

record custom-signal-info {
  checkpoint-id: string,
  payload: list<u8>,
}

record checkpoint-result {
  found: bool,
  state: list<u8>,
  pending-signal: option<signal-info>,
  custom-signal: option<custom-signal-info>,
}

blocking-sleep: func(ms: u64) -> result<_, string>;
get-checkpoint: func(checkpoint-id: string) -> result<option<list<u8>>, string>;
checkpoint: func(checkpoint-id: string, state: list<u8>) -> result<checkpoint-result, string>;
handle-checkpoint-signal: func(signal-type: string) -> result<bool, string>;
record-retry-attempt: func(
  checkpoint-id: string,
  attempt-number: u32,
  error-message: option<string>,
) -> result<_, string>;
durable-sleep-checkpoint: func(
  checkpoint-id: string,
  state: list<u8>,
  ms: u64,
) -> result<_, string>;
```

Current direct stdlib durability helpers:

```wit
agent-cache-key: func(agent-id: u32, source: list<u8>) -> result<list<u8>, string>;
agent-retry-sleep-key: func(
  checkpoint-id: string,
  attempt-number: u32,
) -> result<list<u8>, string>;
agent-retry-delay-ms: func(
  attempt-number: u32,
  total-attempts: u32,
  base-delay-ms: u64,
  max-delay-ms: u64,
  retry-after-ms: option<u64>,
) -> result<u64, string>;
agent-error-info: func(
  code: string,
  message: string,
  category: string,
  severity: string,
  retryable: bool,
  retry-after-ms: option<u64>,
  attributes: option<string>,
) -> result<list<u8>, string>;
record agent-retry-error {
  payload: list<u8>,
  retryable: bool,
  rate-limited: bool,
}
agent-retry-error-info: func(
  code: string,
  message: string,
  category: string,
  severity: string,
  retryable: bool,
  retry-after-ms: option<u64>,
  attributes: option<string>,
) -> result<agent-retry-error, string>;
agent-error-from-info: func(
  agent-id: u32,
  error-info: list<u8>,
) -> result<list<u8>, string>;
delay-duration-ms: func(
  delay-id: u32,
  source: list<u8>,
) -> result<u64, string>;
delay: func(
  delay-id: u32,
  source: list<u8>,
  duration-ms: u64,
) -> result<list<u8>, string>;
```

This is intentionally still a low-level runtime ABI, not a dynamic-linking
scheme. The direct emitter should compile workflow-specific control flow into
the core module and call these statically composed runtime exports at durable
boundaries.

## Error Routing

Direct emitter owns edge routing:

- normal edges;
- `onError` edges;
- conditional edge priority;
- default edge fallback.

Stdlib owns:

- error envelope construction;
- error category extraction;
- edge condition evaluation against the workflow source or `__error`;
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
  `eval-condition`, `process-switch`, `value-switch`, `filter`, `log-event`,
  `log`, `error-event`, `error`, `error-steps`, `group-by`, Agent output,
  validation, connection, cache-key, retry-sleep-key, and error helpers,
  `step-debug-start`, and `step-debug-end`.
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
- Switch config IDs are now in the manifest and the direct stdlib component can
  evaluate value and routing Switch configs through the checked
  `process-switch` and `value-switch` WIT surfaces; parity fixtures cover
  first-match behavior, selected routes, route insertion into the `steps`
  context, array equality shorthand, default output, `BETWEEN`, and `RANGE`.
- GroupBy config IDs are now in the manifest and the direct stdlib component
  can evaluate GroupBy configs through the checked WIT surface; parity fixtures
  cover simple, nested-key, expected-key, null-key, and non-array behavior. The
  direct core now consumes helper-updated `steps` contexts for `Filter`,
  value `Switch`, routing `Switch`, and `GroupBy` workflows before rebuilding
  the source and reaching the selected `Finish`.
- Log IDs are now in the manifest and the direct stdlib component can build
  generated-code-compatible `workflow_log` event payloads and Log step outputs
  through the checked `log-event` and `log` WIT surfaces. The direct core emits
  those payloads through `runtime.custom-event`, rebuilds the source from the
  updated `steps` context, and continues along the normal edge.
- Error IDs are now in the manifest and the direct stdlib component can build
  generated-code-compatible `workflow_error` event payloads and structured
  failure payloads through the checked `error-event` and `error` WIT surfaces.
  The direct core emits the custom event, calls `runtime.fail`, and returns a
  failed `wasi:cli/run` result without posting completion.
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
  workflows by finishing debug event behavior and deciding which currently
  rejected fan-out/no-default routing shapes should stay unsupported.

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
  in `Finish` or `Error` leaves. It evaluates each condition through
  `stdlib.eval-condition` and emits nested Wasm `if` control flow in the
  workflow-specific module.
- `Filter -> Finish`, value `Switch -> Finish`, routing `Switch` trees,
  `GroupBy -> Finish`, and `Log -> Finish` lowering now run end to end. The
  shared direct stdlib returns an updated `steps` context from the step helper,
  and the direct core rebuilds the source before applying the selected final
  `Finish` mapping. Log lowering also emits `workflow_log` through the runtime
  custom-event surface before continuing.
- Terminal `Error` lowering now runs end to end for supported direct-control
  paths. The direct core emits `workflow_error` through the runtime custom-event
  surface, sends the structured failure payload through `runtime.fail`, and
  returns a failed `wasi:cli/run` result.
- Normal/`next` edge-condition lowering now runs end to end for the supported
  direct-control subset. The direct core evaluates conditioned edges through
  `stdlib.eval-condition` in descending priority order and falls back to the
  explicit default edge when no condition matches.
- Compile-time `track_events` now emits generated-code-compatible
  `step_debug_start` and `step_debug_end` events for supported non-Log pure
  JSON/control steps. The stdlib constructs the JSON payloads from the manifest
  and current source envelope; the direct core emits them through
  `runtime.custom-event` before/after each supported step helper or terminal
  action. Log steps continue to emit only `workflow_log`, matching the current
  generated Rust behavior.
- The deliberate production decision for unsupported routing shapes is now
  closed for this phase: parallel fan-out and no-default routing remain outside
  direct Phase 6. They require explicit parallel result aggregation, error
  propagation, or missing-default behavior and should be addressed with loops,
  agents, or runtime lifecycle work instead of being inferred here. Breakpoint
  support moves to Phase 8 because it requires a durable checkpoint/pause ABI,
  not only debug event payloads.

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
7. Implement compile-time `track_events` for supported pure-control steps:
   - lower `step_debug_start`;
   - lower `step_debug_end`;
   - verify event payload parity for representative fixtures.
8. Keep breakpoints rejected until Phase 8:
   - breakpoint behavior depends on persisted checkpoint/pause state;
   - direct runtime WIT does not yet expose a checkpoint API.
9. Keep parallel fan-out/no-default routes rejected until their owning phases:
   - fan-out requires explicit aggregation and error propagation;
   - no-default behavior must be specified per step/routing family.

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

Current status:

- `DirectWorkflowManifest` now serializes Agent-specific manifest entries with
  stable ids, agent id, capability id, optional connection id, retry/timeout
  knobs, and a stable `input_mapping_id`.
- Agent input mappings are serialized into the existing manifest-wide
  `mappings` table with purpose `agent.inputMapping`.
- Component sidecars already collect used agent ids and emit per-agent WIT/WAC
  imports, and the workflow-logic component resolver now includes matching
  per-agent WIT imports in component metadata.
- Non-durable Agent normal-flow lowering now compiles and validates as a
  direct component, including generated Rust retry defaults/overrides and
  steps with a static `connectionId`. Non-durable retry waits use the runtime
  `blocking-sleep` ABI and do not perform checkpoint I/O or retry-attempt
  recording. Durable Agent retry support is enabled for the subset described
  below.
- The shared stdlib WIT now includes `agent-output`, implemented by
  `runtara-workflow-stdlib::direct_json`, to store Agent success outputs using
  the same `steps.<id>` envelope shape as generated Rust code.
- Agent `step_debug_start` uses the Agent input mapping, and
  `step_debug_end` reads the stored Agent step output after source rebuild.
  This covers success debug payloads for the first Agent subset.
- The shared stdlib WIT now also includes `agent-error` and
  `agent-debug-error`. `agent-error` converts WIT `error-info` into the raw
  JSON envelope used by component codegen, then wraps it as
  `Step <id> failed: Agent <agent>::<capability>: <json>`, matching current
  Agent failure formatting. `agent-debug-error` emits the generated
  `{"_error": true, "error": ...}` debug-end output shape.
- The direct manifest now records required Agent capability inputs from the
  compile-time Agent catalog, and `agent-validate-input` validates resolved
  inputs before dispatch. Missing/null fields return the generated Rust
  validation JSON shape and reuse the Agent debug-end failure path.
- `agent-connection-input` injects the same JSON connection fields as generated
  Rust, and direct core writes the current `option<connection-info>` ABI layout
  for static connection ids.
- `error-steps` now builds the generated-code-compatible `onError` source
  context for Agent failures, and direct core lowers Agent `onError` edges with
  condition priority/default routing. Handler branches are emitted as terminal
  direct run plans; an unmatched conditional handler propagates through
  `runtime.fail`.
- `agent-cache-key` now builds the durable Agent idempotency key in stdlib
  using the same workflow id, parent cache prefix, and loop-index suffix rules
  as generated Rust. Direct core injects the compile workflow id into runtime
  variables before building the source, so root direct Agent cache keys no
  longer fall back to the shared `root::` namespace. Direct core has an
  internal `maxRetries = 0` durable Agent checkpoint lowering that uses
  `runtime.get-checkpoint` and `runtime.checkpoint`, and the support gate now
  accepts durable Agent workflows that do not use timeout, compensation, or
  breakpoints.
- Durable Agent retry-loop lowering is now implemented internally for typed WIT
  Agent errors. The direct manifest records the Agent catalog `rateLimited`
  flag, the run plan derives the same default retry counts as generated Rust,
  and the core loop calls `runtime.record-retry-attempt` before retrying. The
  shared stdlib now exposes `agent-error-info`, so retry attempts receive the
  same raw Agent error JSON payload that generated durable Rust records,
  including the camelCase `retryAfterMs` rate-limit hint. The shared stdlib
  also exposes `agent-retry-sleep-key`, and the core lowers typed
  `retryAfterMs` hints to `runtime.durable-sleep-checkpoint` with the generated
  Rust-compatible `rate_limit_wait` state before retry-attempt recording. The
  direct graph manifest now carries `rateLimitBudgetMs`; typed `retryAfterMs`
  retries accumulate raw wait time and continue only while the cumulative total
  stays within that budget. The shared stdlib now also exposes
  `agent-retry-error-info`, which preserves the raw retry payload while
  classifying `RATE_LIMITED`/`HTTP_RATE_LIMITED` codes and permanent
  categories. The direct retry decision uses that classification so
  rate-limited errors without `retryAfterMs` consume the effective base retry
  delay from the same `rateLimitBudgetMs` budget. The shared stdlib also
  exposes `agent-retry-delay-ms`, which centralizes the generated Rust
  exponential backoff and cap formula; direct core calls
  `runtime.durable-sleep` for generic backoff retries and
  `runtime.durable-sleep-checkpoint` for typed `retryAfterMs` waits before
  retry-attempt recording. Direct Agent checkpoint saves now inspect the
  runtime checkpoint result's pending-signal option and call
  `runtime.handle-checkpoint-signal`; handled `cancel`, `pause`, and
  `shutdown` signals stop before `runtime.complete`, while `resume`/unknown
  signals continue. The public support gate accepts this durable Agent subset;
  timeout, compensation, and breakpoints remain rejected, and crash/resume
  differential tests remain pending.
- Structural core Wasm coverage and gated direct execution smokes now cover
  durable and non-durable Agent execution. They prove the cached branch does
  not invoke the Agent or save another checkpoint, that cached raw Agent output
  still feeds the generated-compatible `steps` context, that fresh durable
  execution performs lookup/invoke/save in order, that fresh execution still
  saves only after invoke, and that non-durable Agent execution omits
  checkpoint calls.
- A structural core Wasm test now covers non-durable Agent default retry
  lowering. It proves the direct retry loop uses `runtime.blocking-sleep` and
  omits checkpoint, durable sleep, retry sleep-key, and retry-attempt calls.

Implementation steps:

1. Collect used agents from the graph using existing canonicalization rules.
2. Emit per-agent imports in workflow WIT/component metadata.
3. Extend `wac` generation to instantiate/spread required agents and stdlib.
4. Implement `Agent` lowering:
   - source construction: done for the Agent subset;
   - input mapping: done through `stdlib.apply-mapping`;
   - static `capabilities.invoke`: done for `connection = none` and static
     `connectionId`;
   - success output envelope: done through `stdlib.agent-output`;
   - success ABI result layout: done for the current indirect
     `[pointer, pointer]` invoke lowering;
   - `error-info` to current Agent failure string/debug payload: done;
   - agent input validation: done for required-field missing/null checks;
   - connection JSON injection and WIT `connection-info` envelope: done for
     static `connectionId`;
   - `onError` routing: done for Agent validation/capability failures with
     conditional priority/default handlers;
   - durable no-retry checkpoint lookup/write: internal lowering in place for
     `maxRetries = 0`, public support enabled for the durable Agent subset;
   - durable retry loop and retry-attempt recording: internal lowering in
     place, public support enabled for the durable Agent subset;
   - retry error-message payloads: done through `stdlib.agent-error-info`;
   - typed `retryAfterMs` durable sleep: done through
     `stdlib.agent-retry-sleep-key` and `runtime.durable-sleep-checkpoint`;
   - `rateLimitBudgetMs` propagation and typed `retryAfterMs` cumulative budget:
     done for the durable Agent subset;
   - rate-limit classification and base-delay budget accounting without
     `retryAfterMs`: done for the durable Agent subset;
   - generic exponential backoff sleep: internal lowering in place through
     `stdlib.agent-retry-delay-ms` and `runtime.durable-sleep`, public support
     enabled for the durable Agent subset;
   - pause/cancel/shutdown acknowledgement after checkpoint save: internal
     lowering in place through `runtime.handle-checkpoint-signal`, public
     support enabled for the durable Agent subset;
   - non-durable retry loop parity: done through
     `stdlib.agent-retry-error-info`, `stdlib.agent-retry-delay-ms`, and
     `runtime.blocking-sleep`;
   - fresh/cached host-level Agent execution smokes: done for durable
     lookup/invoke/save, durable cached replay, and non-durable no-checkpoint
     execution;
   - cached-checkpoint replay branch test: done at the emitted core Wasm level
     and in a gated direct execution smoke with preloaded SDK checkpoint state;
   - timeout behavior: pending.
5. Extend `onError` routing beyond Agent when additional failing step types are
   lowered.
6. Preserve current retry policy shape with direct control flow plus
   stdlib/runtime-owned checkpoint, retry-attempt, and sleep behavior.
   This now covers the public durable Agent subset; crash/resume tests still
   need to prove persisted-state parity.

Checkpoint 7:

- Differential tests pass for representative pure and agent fixtures.
- Missing agent component errors identify the agent id and expected path.
- Agent error envelopes match current behavior.
- Connection-using agent fixtures pass in an integration environment.

Rollback:

- Direct mode can re-gate durable Agent workflows while keeping the
  non-durable Agent subset available.

### Phase 8: Runtime Lifecycle and Durability ABI

Goal: replace macro-hidden `#[resilient]` behavior with explicit runtime ABI
without changing workflow semantics.

Implementation steps:

1. Specify checkpoint/runtime WIT:
   - checkpoint lookup/write: done;
   - signal-aware checkpoint result: done;
   - checkpoint signal acknowledgement/suspension helper: done for
     `cancel`/`pause`/`shutdown`;
   - retry-attempt recording: done;
   - checkpointed durable sleep: done;
   - heartbeat: already exposed;
   - cancellation check: already exposed;
   - stable resume-from-checkpoint lowering: pending per step family.
2. Implement stdlib/runtime functions using the existing SDK behavior.
3. Generate stable cache keys matching current behavior:
   - workflow id: done for Agent cache keys and injected by direct core;
   - step id: done for Agent cache keys;
   - loop indices: done for Agent cache keys;
   - child cache prefixes: done for Agent cache keys;
   - retry sleep scope: done for Agent typed `retryAfterMs`;
   - graph `rateLimitBudgetMs` propagation: done for Agent typed
     `retryAfterMs`;
   - rate-limit classification and base-delay budget scope: done for Agent
     typed WIT errors;
   - generic backoff sleep scope: done for Agent typed WIT errors.
4. Migrate durable `Agent`:
   - no-retry checkpoint lookup/write: internal lowering done;
   - retry loop and retry-attempt recording: internal lowering done;
   - retry error-message payloads: internal lowering done;
   - typed `retryAfterMs` durable sleep: internal lowering done;
   - typed `retryAfterMs` cumulative budget: internal lowering done;
   - rate-limit classification and no-`retryAfterMs` budget accounting:
     internal lowering done;
   - generic backoff sleep parity: internal lowering done;
   - pause/cancel/shutdown acknowledgement parity after Agent checkpoint save:
     internal lowering done;
   - crash/resume differential tests: pending.
5. Migrate `Delay`:
   - manifest config records: done;
   - immediate and dynamic `durationMs` mapping: done through
     `stdlib.delay-duration-ms`;
   - generated Rust-compatible step output shape
     (`steps.<stepId>.duration_ms`): done through `stdlib.delay`;
   - durable sleep: done through
     `runtime.durable-sleep-checkpoint(stepId, [], durationMs)`;
   - non-durable blocking sleep parity: done through
     `runtime.blocking-sleep(durationMs)`;
   - public support gate: enabled for durable and non-durable Delay without
     breakpoints;
   - Delay breakpoints: pending and gated;
   - host-level crash/resume differential tests: pending.
6. Add crash/resume tests:
   - resume after checkpoint: structural core replay test and gated host-level
     cached Agent replay smoke done; full differential test pending;
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

- Direct mode can re-gate durable Agent workflows until this checkpoint passes.

### Phase 9: Split and While

Goal: support loop and collection control flow.

Implementation steps:

1. Implement sequential `Split` first.
   - config manifest records with input/output schemas and nested graph link:
     done;
   - stdlib split-input/source helpers: pure Rust helpers and WIT exports done
     for null, single-value, batching, indexed item access, result
     accumulation, `_loop_indices`, `_item`, `_index`, `_scope_id`, extra
     variables, and result envelopes; direct lowering pending;
   - direct loop lowering: pending.
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
