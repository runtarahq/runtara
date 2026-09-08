# runtara-dsl

[![Crates.io](https://img.shields.io/crates/v/runtara-dsl.svg)](https://crates.io/crates/runtara-dsl)
[![Documentation](https://docs.rs/runtara-dsl/badge.svg)](https://docs.rs/runtara-dsl)

Single source of truth for Runtara's workflow DSL types — the Rust structs that define workflows, steps, and value mappings.

## What it is

A typed representation of a workflow execution graph: `Workflow`, `ExecutionGraph`, `Step` (Agent, Conditional, Split, Switch, While, Log, Error, ...), and `MappingValue` (`Reference`, `Immediate`, `Composite`, `Template`). Serde handles JSON round-trips, `schemars` auto-generates the matching JSON Schema at build time (pinned to `DSL_VERSION`, currently `3.0.0`), and step metadata is exposed through a static registry so the schema stays in sync with the step structs themselves. The public entry points are `parse_workflow`, `parse_execution_graph`, `get_step_types`, plus helpers like `ExecutionGraph::get_terminal_errors` for introspection.

## Using it standalone

Add it to a crate that needs to read, validate, or emit workflow JSON:

```toml
[dependencies]
runtara-dsl = "4.0"
serde_json = "1"
```

```rust
use runtara_dsl::{parse_workflow, MemoryTier};

let json = serde_json::from_str(r#"{"executionGraph":{"entryPoint":"start","steps":{},"executionPlan":[],"variables":{},"inputSchema":{},"outputSchema":{}}}"#)?;
let workflow = parse_workflow(&json)?;
assert_eq!(workflow.memory_tier.unwrap_or(MemoryTier::XL).total_memory_bytes(), 256 * 1024 * 1024);
```

Enable the `utoipa` feature if you need `ToSchema` derives for OpenAPI generation.

## Execution labels

A top-level Finish may set optional `runLabel` metadata independently of its
`inputMapping` output:

```json
{
  "id": "finish",
  "stepType": "Finish",
  "runLabel": { "valueType": "template", "value": "Order/{{ data.orderId }} [done]" },
  "inputMapping": { "success": { "valueType": "immediate", "value": true } }
}
```

Literal strings and references are supported too. The resolved string is trimmed
of surrounding spaces and may contain up to 250 ASCII letters, digits, spaces,
dots, dashes, forward slashes, parentheses, or square brackets. Omitted, null,
and empty labels mean no label. A supplied nonempty label must contain at least
one letter or digit; whitespace-only, punctuation-only, and invisible characters
are invalid. Duplicate labels are allowed. Invalid literals
fail workflow validation; invalid dynamic results (including evaluation errors)
are ignored and Finish completes normally without a label. Labels longer than
250 characters are truncated, then trailing spaces are removed. The retained
text must still contain a letter or digit.

The label is saved with successful completion and replaces the workflow name in
execution lists. Executions that have not reached Finish retain their workflow
name. Finish steps inside Split, While, or onWait subgraphs cannot set labels;
inline child workflows and workflow agent capabilities cannot rename their parent.
The execution list supports case-insensitive literal substring `search` and an
exact `runLabel` filter, both applied before pagination and counting.

## Inside Runtara

- Consumed by `runtara-workflows` (compiler/executor), `runtara-agents` (capability metadata), and `runtara-server` (REST validation + OpenAPI surface).
- Also pulled in by `runtara-core`, `runtara-connections`, `runtara-workflow-stdlib`, `runtara-text-parser`, `runtara-environment`, and `runtara-test-harness` — nearly every runtime crate touches these types.
- Depends only on `serde`, `serde_json`, and `schemars`; `utoipa` is optional.
- Step type metadata is defined in `step_registration.rs`, so `get_step_types()` and generated schema never drift from the actual enum variants.
- Runs everywhere the rest of Runtara runs — native host, WASI agents, and build scripts (the schema JSON in `specs/dsl/v3.0.0/schema.json` is regenerated from these types).

## License

AGPL-3.0-or-later.
