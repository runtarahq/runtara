# runtara-dsl

[![Crates.io](https://img.shields.io/crates/v/runtara-dsl.svg)](https://crates.io/crates/runtara-dsl)
[![Documentation](https://docs.rs/runtara-dsl/badge.svg)](https://docs.rs/runtara-dsl)

Single source of truth for Runtara's workflow DSL types — the Rust structs that define workflows, steps, and value mappings.

## What it is

A typed representation of a workflow execution graph: `Workflow`, `ExecutionGraph`, `Step` (Agent, Conditional, Split, Switch, While, Log, Error, ...), and `MappingValue` (`Reference`, `Immediate`, `Composite`, `Template`). Serde handles JSON round-trips, `schemars` auto-generates the matching JSON Schema at build time (pinned to `DSL_VERSION`, currently `3.0.0`), and step metadata is collected via `inventory` so the schema stays in sync with the step structs themselves. The public entry points are `parse_workflow`, `parse_execution_graph`, `get_step_types`, plus helpers like `ExecutionGraph::get_terminal_errors` for introspection.

## Using it standalone

Add it to a crate that needs to read, validate, or emit workflow JSON:

```toml
[dependencies]
runtara-dsl = "1.8"
serde_json = "1"
```

```rust
use runtara_dsl::{parse_workflow, MemoryTier};

let json = serde_json::from_str(r#"{"executionGraph":{"entryPoint":"start","steps":{},"executionPlan":[],"variables":{},"inputSchema":{},"outputSchema":{}}}"#)?;
let workflow = parse_workflow(&json)?;
assert_eq!(workflow.memory_tier.unwrap_or(MemoryTier::XL).total_memory_bytes(), 256 * 1024 * 1024);
```

Enable the `utoipa` feature if you need `ToSchema` derives for OpenAPI generation.

## Inside Runtara

- Consumed by `runtara-workflows` (compiler/executor), `runtara-agents` (capability metadata), and `runtara-server` (REST validation + OpenAPI surface).
- Also pulled in by `runtara-core`, `runtara-connections`, `runtara-workflow-stdlib`, `runtara-text-parser`, `runtara-environment`, and `runtara-test-harness` — nearly every runtime crate touches these types.
- Depends only on `serde`, `serde_json`, `schemars`, and `inventory`; `utoipa` is optional.
- Step struct definitions register themselves via `inventory` in `step_registration.rs`, so `get_step_types()` and generated schema never drift from the actual enum variants.
- Runs everywhere the rest of Runtara runs — native host, WASI agents, and build scripts (the schema JSON in `specs/dsl/v3.0.0/schema.json` is regenerated from these types).

## License

AGPL-3.0-or-later.
