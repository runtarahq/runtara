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
- `lifecycle` defines pure command replacement, acknowledgment, parked cancellation, parking, and interruption policy.
- `persistence` defines records, queries, conditional updates, wake claims, and retention obligations through `Persistence`.
- `instance_handlers` implements registration, checkpoints, sleep, events, and signal delivery over that trait.
- `error` provides storage-neutral errors and transport-independent classifications.

Hosts pass configuration explicitly, including handler concurrency limits through `InstanceHandlerState::with_limits`. Core never reads the process environment; the host owns deployment variable names, defaults, precedence, and transport shutdown settings.

Event subtypes and payload keys are opaque producer-defined strings. `EventVocabulary` requires distinct opening and closing subtypes. Each backend validates any additional restrictions required by its query implementation.

The `test-support` feature exposes an in-memory backend, handler mocks, and a shared conformance suite. Backend implementations should run that suite against their own store; tests in core need no external services.

## Lifecycle policy

Core owns transition decisions; persistence implementations own atomic application;
environment owns execution, launch scheduling, and recovery. Call policy functions
against state read under the backend's lock or transaction, then apply all effects
before releasing it. Do not read, decide, and write in separate operations.

`Decision` distinguishes `Rejected`, `AlreadyApplied`, and `Applied(Transition)`.
`Change::Keep`, `Clear`, and `Set` distinguish preserving metadata from removing or
replacing it. Backends encode those values, use one operation timestamp, persist
status/deadlines/events/acknowledgment together, and report newly applied completion
metrics after committing. Repeated receipts produce no durable effects.

PostgreSQL locks instances before commands, ordered by instance ID for batches.
Recovery uses bounded candidate selection with `SKIP LOCKED`, re-evaluates policy
against locked commands, and applies equal effects with batched writes in one
transaction. Candidate-query predicates are an optimization; they do not replace
policy evaluation. Memory evaluates and applies the same policy under one mutex.

A matching cancellation acknowledgment retains the existing ability to override
completed/failed status when cancellation was not honored. Parked cancellation
only applies to suspended instances. These are distinct operations with distinct
guards. Parking only applies to running instances and commits its deadline with
suspension metadata; a signal arriving before parking is checked by environment
immediately afterward, while later arrivals can observe the suspended state.

Custom-signal replay, explicit host resume, and guest component interfaces keep
their existing contracts. Core's host dependencies are not added to WASM guests.

## Adapter migration

Backend implementers must supply `apply_lifecycle_command` and `park_instance`,
returning the core policy's typed `Decision`. The existing `acknowledge_signal`
method is a boolean compatibility wrapper: applied and already-applied receipts
both return true. HTTP acknowledgment responses and WIT runtime `0.2.0` are
unchanged by this refactor; no new schema migration is required.

Persistence records, status filters, completion parameters, event filters, and lifecycle signal operations now use `domain` enums. Stored event types include `Started` and legacy `Progress`, in addition to incoming instance events. PostgreSQL encoding and checked decoding live in `runtara-store-postgres::encoding`; core's database string mappers have been removed.

`CoreError::DatabaseError` is now `CoreError::PersistenceError`, with code `PERSISTENCE_ERROR`. The blanket conversion from JSON errors has been removed: callers must classify failures in context. The server's HTTP route error codes and existing database enum labels remain unchanged.

The `config` module has been removed. `RuntimeOverrides` now lives in `runtara_server::config`, alongside the host's environment loading and `ConfigError`. Cleanup callers use the pure `runtara_environment::config::parse_enabled(Option<&str>)` parser instead of core's environment-reading helper.

## License

AGPL-3.0-or-later.
