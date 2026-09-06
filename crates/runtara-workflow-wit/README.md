# runtara-workflow-wit

Canonical WIT contracts for direct-emitted workflow components.

This crate intentionally separates workflow semantics from runtime lifecycle:

- `runtara:workflow-stdlib/json@0.1.0` owns reusable JSON semantics such as
  manifest initialization, source construction, mappings, conditions, switch
  routing, filtering, logging payloads, structured error payloads, and grouping.
- `runtara:workflow-runtime/runtime@0.3.0` owns SDK/runtime lifecycle calls such
  as input loading, completion, failure, events, cancellation, and durable
  sleep.

Both are composed statically with workflow-logic and agent components into one
final `workflow.wasm`.

Runtime 0.3.0 adds `signal-id` to `custom-signal-info`, separate from its
`checkpoint-id` address. Reads are retained and writes replace the value with
a new identity. Guest `resume` is retired; resumption belongs to the host.
Rebuild components and recompile workflow artifacts for this ABI change.

The runtime 0.2.0 contract added `command-id` to lifecycle signals and requires it
when acknowledging a checkpoint signal. Recompile workflow artifacts and rebuild
the shared runtime component when upgrading from 0.1.0. The instance HTTP API
likewise requires the `command_id` returned by polling or checkpointing in
`POST /api/v1/instances/{instance_id}/signals/ack`; `success: false` means the
receipt is stale and no lifecycle transition was applied. Retrying an accepted
receipt succeeds without applying the transition again.
