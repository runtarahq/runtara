# runtara-dsl

Single source of truth for Runtara's workflow DSL types — the Rust structs that define workflows, steps, and value mappings.

## What it is

A typed representation of a workflow execution graph: `Workflow`, `ExecutionGraph`, `Step` (Agent, Conditional, Split, Switch, While, Log, Error, ...), and `MappingValue` (`Reference`, `Immediate`, `Composite`, `Template`). Serde handles JSON round-trips, `schemars` auto-generates the matching JSON Schema at build time (stamped with `DSL_VERSION`), and step metadata is exposed through a static registry so the schema stays in sync with the step structs themselves. The public entry points are `parse_workflow`, `parse_execution_graph`, `get_step_types`, plus helpers like `ExecutionGraph::get_terminal_errors` for introspection.

## Using it standalone

It is not published to crates.io; depend on it by path inside this workspace
(or by git from outside it):

```toml
[dependencies]
runtara-dsl = { path = "../runtara-dsl" }
serde_json = "1"
```

```rust
use runtara_dsl::parse_workflow;

let json = serde_json::from_str(
    r#"{"executionGraph":{"name":"hello","entryPoint":"done","steps":{"done":{"stepType":"Finish","id":"done"}}}}"#,
)?;
let workflow = parse_workflow(&json)?;
assert_eq!(workflow.execution_graph.entry_point, "done");
```

Enable the `utoipa` feature if you need `ToSchema` derives for OpenAPI generation.

## Execution labels

Pass optional `runLabel` alongside `inputs` when starting an execution:

```json
{
  "inputs": { "data": { "orderId": "order_123" }, "variables": {} },
  "runLabel": "order_123"
}
```

Labels contain 1–1024 printable ASCII bytes with at least one non-space character.
Accepted strings are preserved exactly, including case and surrounding spaces.
Empty, control-character, non-ASCII, and oversized labels are rejected. Omission
or null starts an unlabeled execution. Labels are non-unique references, separate
from the instance ID and idempotency key. Replaying an idempotency key with a
conflicting label fails without modifying the original execution.

Labels are stored at instance creation and remain unchanged through suspension,
resume, retries, and completion. Execution lists display the label when present
and otherwise the workflow name. Exact `runLabel` and other execution filters
apply before pagination and counting.

`Finish` only maps outputs. It does not accept `runLabel`, and inline child
workflows cannot label the parent execution. Independently started executions
can receive their own label through the start API.

## Inside Runtara

- Consumed by `runtara-workflows` (compiler/executor), `runtara-agents` (capability metadata), and `runtara-server` (REST validation + OpenAPI surface).
- Also pulled in by `runtara-core`, `runtara-connections`, `runtara-workflow-stdlib`, `runtara-text-parser`, `runtara-environment`, and `runtara-test-harness` — nearly every runtime crate touches these types.
- Depends only on `serde`, `serde_json`, and `schemars`; `utoipa` is optional.
- Step type metadata is defined in `step_registration.rs`, so `get_step_types()` and generated schema never drift from the actual enum variants.
- Runs everywhere the rest of Runtara runs — native host, WASI agents, and build scripts. The JSON Schema is regenerated from these types at build time and served at `GET /api/runtime/specs/dsl` (the changelog at `/specs/dsl/changelog`).

## License

AGPL-3.0-or-later.
