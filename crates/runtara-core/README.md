# runtara-core

Durable execution engine for Runtara: checkpoints, signals, durable sleep, and instance events backed by PostgreSQL.

[![crates.io](https://img.shields.io/crates/v/runtara-core.svg)](https://crates.io/crates/runtara-core)
[![docs.rs](https://docs.rs/runtara-core/badge.svg)](https://docs.rs/runtara-core)
[![License](https://img.shields.io/crates/l/runtara-core.svg)](LICENSE)

## What it is

`runtara-core` is the host-side execution engine that workflow instances talk to in order to persist state and progress durably. The `persistence` module defines the `Persistence` trait (implemented by `PostgresPersistence`) covering instances, checkpoints, events, and signals. The `instance_handlers` module implements the instance protocol (register, checkpoint, sleep, events, signal poll/ack) as plain async functions over that trait. The `migrations` module ships SQL migrations so embedders can set up the schema.

It is a library, not a service: no HTTP, no sockets, no binary. A host picks the transport. `runtara-server` serves these handlers over HTTP on the instance port for guests using the SDK's HTTP backend; `runtara-environment` calls them in-process for guests composed against the runtime as a host import, which is the default.

## Using it

```toml
[dependencies]
runtara-core = "8.7"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use std::sync::Arc;
use runtara_core::instance_handlers::{InstanceHandlerState, handle_checkpoint};
use runtara_core::persistence::{Persistence, PostgresPersistence};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = PgPoolOptions::new().connect(&std::env::var("RUNTARA_DATABASE_URL")?).await?;
    runtara_core::migrations::run_postgres(&pool).await?;
    let persistence: Arc<dyn Persistence> = Arc::new(PostgresPersistence::new(pool));

    let state = Arc::new(InstanceHandlerState::new(persistence));
    // Call the handlers directly, or wrap `state` in a transport of your own.
    let _ = &state;
    Ok(())
}
```

Requires a reachable PostgreSQL database.

## Inside Runtara

- `runtara-server` owns the instance HTTP API (`core_runtime` module), including the drain that lets an instance mid-checkpoint finish writing.
- `runtara-environment` shares the `Persistence` trait and calls `instance_handlers` directly, never over HTTP.
- `runtara-sdk` uses it via the optional `embedded` feature for in-process tests that skip the HTTP hop.
- Depends on `sqlx` (Postgres) and `tokio`. No web framework.
- The `test-support` feature exposes the in-memory `Persistence` mock for downstream handler tests.
- Primary integration point is the `Persistence` trait — environment and SDK both program against it.
- Runs in: native host (Tokio + sqlx).

## License

AGPL-3.0-or-later.
