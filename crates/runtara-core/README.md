# runtara-core

Durable execution engine for Runtara: checkpoints, signals, durable sleep, and instance events backed by PostgreSQL or SQLite.

[![crates.io](https://img.shields.io/crates/v/runtara-core.svg)](https://crates.io/crates/runtara-core)
[![docs.rs](https://docs.rs/runtara-core/badge.svg)](https://docs.rs/runtara-core)
[![License](https://img.shields.io/crates/l/runtara-core.svg)](LICENSE)

## What it is

`runtara-core` is the host-side execution engine that workflow instances talk to in order to persist state and progress durably. The `persistence` module defines the `Persistence` trait (with `PostgresPersistence` and `SqlitePersistence` impls) covering instances, checkpoints, events, and signals. The `instance_handlers` and `server` modules expose the instance protocol over HTTP (register, checkpoint, sleep, events, signal poll/ack), and `runtime::CoreRuntime` bundles it into an embeddable service. The `migrations` module ships SQL migrations so embedders can set up the schema, and `compensation` provides saga-style rollback primitives.

## Using it standalone

```toml
[dependencies]
runtara-core = "4.0"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use std::sync::Arc;
use runtara_core::config::Config;
use runtara_core::persistence::{Persistence, PostgresPersistence};
use runtara_core::runtime::CoreRuntime;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let pool = PgPoolOptions::new().connect(&config.database_url).await?;
    runtara_core::migrations::run_postgres(&pool).await?;
    let persistence: Arc<dyn Persistence> = Arc::new(PostgresPersistence::new(pool));

    let runtime = CoreRuntime::builder()
        .persistence(persistence)
        .bind_addr(config.http_addr)
        .max_concurrent_instances(config.max_concurrent_instances)
        .build()?
        .start()
        .await?;

    tokio::signal::ctrl_c().await?;
    runtime.shutdown().await?;
    Ok(())
}
```

Requires a reachable PostgreSQL or SQLite database via `RUNTARA_DATABASE_URL`. Disable the default `server` feature if you only need the persistence/migrations library surface.

## Inside Runtara

- Consumed by `runtara-server` (binary that links core with `server` feature) and `runtara-environment` (shares the `Persistence` trait directly, not over HTTP).
- `runtara-sdk` uses it via the optional `embedded` feature for in-process tests that skip the HTTP hop.
- Depends on `sqlx` (Postgres + SQLite), `tokio`, and `axum` for the instance HTTP server on port 8001.
- Primary integration point is the `Persistence` trait — environment and SDK both program against it.
- Runs in: native host (Tokio + sqlx). Ships as both a library and an optional binary (`[[bin]] runtara-core`, gated on the `server` feature).

## License

AGPL-3.0-or-later.
