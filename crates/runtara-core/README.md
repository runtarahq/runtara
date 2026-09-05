# runtara-core

Durable execution semantics for Runtara: instances, checkpoints, signals, sleep, and events over a host-provided persistence backend.

## Using core

Core is a library. The host supplies storage and transport; `runtara-server` exposes the instance protocol over HTTP, and embedded callers invoke the same handlers directly.

```rust
use std::sync::Arc;
use runtara_core::instance_handlers::InstanceHandlerState;
use runtara_core::persistence::Persistence;

fn handlers(persistence: Arc<dyn Persistence>) -> InstanceHandlerState {
    InstanceHandlerState::new(persistence)
}
```

A host choosing PostgreSQL obtains its implementation and migrations from `runtara-store-postgres`:

```rust
use std::sync::Arc;
use runtara_core::instance_handlers::InstanceHandlerState;
use runtara_store_postgres::{migrations, PostgresPersistence};

async fn postgres_handlers(pool: sqlx::PgPool) -> anyhow::Result<InstanceHandlerState> {
    migrations::run_postgres(&pool).await?;
    Ok(InstanceHandlerState::new(Arc::new(PostgresPersistence::new(pool))))
}
```

That host depends on `runtara-core`, `runtara-store-postgres`, `sqlx`, and `anyhow`. Core itself has no database-driver dependency and ships no migrations.

## Contracts

- `domain` defines typed instance statuses, lifecycle signals, and timeline events. Storage encodings and wire spellings belong to adapters.
- `persistence` defines records, queries, conditional updates, wake claims, and retention obligations through `Persistence`.
- `instance_handlers` implements registration, checkpoints, sleep, events, and signal delivery over that trait.
- `error` provides storage-neutral errors and transport-independent classifications.
- `config` provides runtime overrides that hosts apply when constructing their runtime.

Event subtypes and payload keys are opaque producer-defined strings. `EventVocabulary` requires distinct opening and closing subtypes. Each backend validates any additional restrictions required by its query implementation.

The `test-support` feature exposes an in-memory backend, handler mocks, and a shared conformance suite. Backend implementations should run that suite against their own store; tests in core need no external services.

## Adapter migration

Persistence records, status filters, completion parameters, event filters, and lifecycle signal operations now use `domain` enums. Stored event types include `Started` and legacy `Progress`, in addition to incoming instance events. PostgreSQL encoding and checked decoding live in `runtara-store-postgres::encoding`; core's database string mappers have been removed.

`CoreError::DatabaseError` is now `CoreError::PersistenceError`, with code `PERSISTENCE_ERROR`. The blanket conversion from JSON errors has been removed: callers must classify failures in context. The server's HTTP route error codes and existing database enum labels remain unchanged.

## License

AGPL-3.0-or-later.
