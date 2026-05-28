# runtara-workflow-wit

Canonical WIT contracts for direct-emitted workflow components.

This crate intentionally separates workflow semantics from runtime lifecycle:

- `runtara:workflow-stdlib/json@0.1.0` owns reusable JSON semantics such as
  manifest initialization, source construction, mappings, conditions, switch
  routing, filtering, logging payloads, and grouping.
- `runtara:workflow-runtime/runtime@0.1.0` owns SDK/runtime lifecycle calls such
  as input loading, completion, failure, events, cancellation, and durable
  sleep.

Both are composed statically with workflow-logic and agent components into one
final `workflow.wasm`.
