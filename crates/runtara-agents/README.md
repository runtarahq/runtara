# runtara-agents

[![Crates.io](https://img.shields.io/crates/v/runtara-agents.svg)](https://crates.io/crates/runtara-agents)
[![Documentation](https://docs.rs/runtara-agents/badge.svg)](https://docs.rs/runtara-agents)
[![License](https://img.shields.io/crates/l/runtara-agents.svg)](LICENSE)

Pre-compiled agent implementations — HTTP, CSV/XML, SFTP, XLSX, crypto, transforms, and platform integrations — that runtara workflows link against.

## What it is

A library of reusable "agent" capabilities that back the steps in a runtara workflow. Each agent module (e.g. `http`, `csv`, `transform`, `sftp`, `integrations::shopify`) exposes executors registered via the `#[capability]` macro from `runtara-agent-macro` and dispatched through an `inventory`-backed registry. Consumers call `runtara_agents::registry::execute_capability(agent_id, capability_id, inputs)` — the library itself has no runtime loop, just synchronous capability functions plus shared `types` (e.g. `FileData`, `AgentError`) and `connections` scaffolding. Feature flags gate platform support: `native` pulls in C-dependent agents (SFTP via `ssh2`, XLSX via `calamine`, ZIP), while `wasi` and `wasm-js` swap in WASM-compatible transport; `integrations` enables SaaS connectors (Shopify, OpenAI, Bedrock, Stripe, HubSpot, Slack, Mailgun, S3).

## Using it standalone

```toml
[dependencies]
runtara-agents = { version = "1.8", default-features = false, features = ["integrations"] }
```

```rust
use runtara_agents::registry::execute_capability;
use serde_json::json;

let out = execute_capability(
    "utils",
    "random-double",
    json!({ "min": 0.0, "max": 1.0 }),
)?;
```

Pick one platform feature: `native` (default) for servers and CLIs, `wasi` for `wasm32-wasip2` guests, or `wasm-js` for browser/Node. The `generate_dsl_spec` binary emits the capability catalog as JSON for tooling.

## Inside Runtara

- Consumed by `runtara-workflows`, `runtara-workflow-stdlib`, `runtara-server`, and `runtara-environment` — the stdlib re-exports feature flags so downstream workflow binaries pick the right target.
- Built on `runtara-dsl` (capability metadata, error model) and `runtara-agent-macro` (the `#[capability]` / `CapabilityOutput` derives that register executors into `inventory`).
- Key integration point: `runtara_dsl::agent_meta::execute_capability`, which `registry.rs` thinly wraps — everything downstream dispatches through that single entry point.
- Runs in both native and WASM guests: the `wasi` and `wasm-js` features gate `ssh2`, `openssl`, `calamine`, and `zip` out so the crate compiles cleanly for `wasm32-wasip2` and browser targets.
- HTTP goes through the workspace `runtara-http` abstraction so the same agent code works across transports.

## License

AGPL-3.0-or-later.
