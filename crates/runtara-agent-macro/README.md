# runtara-agent-macro

[![Crates.io](https://img.shields.io/crates/v/runtara-agent-macro.svg)](https://crates.io/crates/runtara-agent-macro)
[![Documentation](https://docs.rs/runtara-agent-macro/badge.svg)](https://docs.rs/runtara-agent-macro)
[![License](https://img.shields.io/crates/l/runtara-agent-macro.svg)](LICENSE)

Procedural macros for declaring runtara agent capabilities, input/output schemas, connection types, and step metadata.

## What it is

A `proc-macro` crate that exposes one attribute macro and four derives: `#[capability]` marks a function as an agent capability (emitting a `CapabilityMeta` static plus a JSON-coercing executor wrapper), `#[derive(CapabilityInput)]` and `#[derive(CapabilityOutput)]` lift struct fields into `InputTypeMeta` / `OutputTypeMeta`, `#[derive(ConnectionParams)]` describes a connection type with auth type, category, and OAuth config, and `#[derive(StepMeta)]` registers DSL step types with `schemars`-derived schemas. The emitted metadata types all live in `runtara-dsl::agent_meta` (not here, to sidestep proc-macro crate limits), and on non-WASM targets each static is also submitted to `inventory` for runtime discovery. In practice every consumer inside the workspace is `runtara-agents`.

## Using it standalone

The macros generate paths like `runtara_dsl::agent_meta::CapabilityMeta` and `inventory::submit!`, so any direct consumer must also depend on `runtara-dsl` and `inventory`:

```toml
[dependencies]
runtara-agent-macro = "1.8"
runtara-dsl         = "1.8"
inventory           = "0.3"
serde               = { version = "1", features = ["derive"] }
serde_json          = "1"
```

```rust
use runtara_agent_macro::{capability, CapabilityInput};

#[derive(CapabilityInput, serde::Deserialize)]
pub struct AddInput { pub a: i64, pub b: i64 }

#[capability(module = "math", id = "add", description = "Sum two ints")]
pub fn add(i: AddInput) -> Result<i64, String> { Ok(i.a + i.b) }
```

Most downstream code should just depend on `runtara-agents`, which already pulls these macros in as a transitive dependency.

## Inside Runtara

- Primary consumer: `runtara-agents` — every built-in agent module (`http`, `sftp`, `xlsx`, `csv`, `crypto`, `datetime`, `transform`, `text`, `file`, `compression`, `xml`, `utils`) is written with `#[capability]` plus the input/output derives.
- `runtara-agents/src/agents/extractors/*` uses `#[derive(ConnectionParams)]` for the built-in connection types (`http_bearer`, `http_api_key`, `sftp`).
- Generated metadata targets types in `runtara-dsl::agent_meta` and is indexed via `inventory::submit!`, gated on `not(target_family = "wasm")`.
- Deps: `syn` 2 (full/parsing/extra-traits), `quote`, `proc-macro2`, `darling` 0.20.
- The `#[capability]` executor wrapper normalizes errors into JSON envelopes (`code` / `message` / `category` / `severity`) so the `#[durable]` layer can make retry decisions.
- Runs in: proc-macro at compile time.

## License

AGPL-3.0-or-later.
