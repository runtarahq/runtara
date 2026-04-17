# runtara-workflow-stdlib

[![Crates.io](https://img.shields.io/crates/v/runtara-workflow-stdlib.svg)](https://crates.io/crates/runtara-workflow-stdlib)
[![Documentation](https://docs.rs/runtara-workflow-stdlib/badge.svg)](https://docs.rs/runtara-workflow-stdlib)
[![License](https://img.shields.io/crates/l/runtara-workflow-stdlib.svg)](LICENSE)

The single crate that every generated runtara workflow binary links against.

## What it is

`runtara-workflow-stdlib` is a thin umbrella crate that bundles everything a compiled workflow binary needs at runtime: agents (`runtara-agents`), the durable execution SDK (`runtara-sdk`), AI helpers (`runtara-ai`), plus a few pieces of glue that only make sense for generated code — condition helpers, switch-step output processing, Jinja-style template rendering, child scenario input validation, a capability dispatch table, and a connection-fetching client. Its public shape is a `prelude` module that re-exports the types codegen emits (`RuntaraSdk`, `durable`, `register_sdk`, `fetch_connection`, `validate_child_inputs`, etc.) and top-level re-exports of `runtara_agents`, `runtara_ai`, `runtara_sdk`, `serde`, `serde_json`, and `tracing`. Feature flags (`native`, `wasi`, `wasm-js`, `telemetry`) pick the target: workflows compile to WASI components in production, native for tests.

## Using it standalone

Not really intended for hand-written code — the API surface is shaped by what `runtara-workflows` codegen emits. If you want to experiment, pick a target and pull the prelude in:

```toml
[dependencies]
runtara-workflow-stdlib = { version = "1.8", default-features = false, features = ["wasi"] }
```

```rust
use runtara_workflow_stdlib::prelude::*;

let sdk = RuntaraSdk::new(/* transport */)?;
register_sdk(sdk);
durable("step-1", || Ok(serde_json::json!({"ok": true})))?;
```

For real workflows, author DSL and let `runtara-workflows` generate the Rust.

## Inside Runtara

- Consumed by `runtara-workflows` codegen — every generated workflow `main.rs` starts with `use runtara_workflow_stdlib::prelude::*;` (the crate name is overridable via `RUNTARA_STDLIB_NAME`).
- Linked into `runtara-test-harness` (embedded test runner) and `runtara-server` (workflow compilation at the edge).
- Core deps: `runtara-agents` (integration library), `runtara-sdk` (durable execution protocol), `runtara-ai` (AI Agent steps); optional OpenTelemetry stack behind the `telemetry` feature.
- `connections::fetch_connection` calls the connection service over HTTP using `runtara-http`, keyed by `tenant_id`/`connection_id` with optional rate-limit state passthrough.
- Runs primarily as a WASI guest (`wasm32-wasip2`) inside the runtara environment; the `native` feature exists for local testing and for agents with C deps (xlsx, sftp, compression).
- `dispatch` module is designed for static capability tables so product stdlibs can override agent dispatch without dynamic registration.

## License

AGPL-3.0-or-later.
