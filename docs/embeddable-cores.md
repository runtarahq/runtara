# Embeddable cores — high‑level notes

Brain dump of the current design direction for running runtara-core in embedded and hybrid setups.

## Goals
- Allow apps to host the durable runtime in-process with a self-contained store (SQLite/libsql).
- Keep the existing protocols/handlers intact; only swap transport/storage and discovery.
- Make SDKs storage/transport pluggable so other languages can embed the same semantics.

## Modes
- **Server mode**: core + environment as today; Postgres; QUIC for instance/management.
- **Embedded mode**: core runtime + SQLite/libsql in-process; no environment involvement; instances run locally; signals via SDK API into local store.
- **Hybrid (local durability + environment)**: embedded core still runs instance/management listeners; environment connects to it for lifecycle and signals; durability remains local (SQLite/libsql).

## SDK shape
- Introduce traits: `Storage` (checkpoints, events, status) and `SignalTx`.
- Adapters:
  - `EmbeddedStorage`/`EmbeddedSignalTx`: sqlx SQLite/libsql, no QUIC.
  - `RemoteStorage`/`RemoteSignalTx`: current QUIC clients.
- Config enum: `SdkMode::Embedded { sqlite_url, migrate_on_start, … } | SdkMode::Server { core_addr, tls, … }`.
- Single API: `send_signal(instance_id, Signal)` routed to the chosen adapter. Instances remain unaware of environment.
- Sleep remains in-process (`tokio::sleep`); no wake scheduler needed unless durable sleep returns.

## Core ↔ Environment discovery
- Core registers with environment on startup: payload includes addresses, version, heartbeat TTL, and capability flags.
- Add flag `embedded` (or `accept_new_instances`): signals/status allowed, but environment must not schedule new instances onto embedded cores unless explicitly permitted.
- Environment keeps a core registry; scheduling filters out `embedded=true` cores for new starts. Existing bound instances can still receive signals.
- Optional guard: core rejects new-instance creation when flagged as embedded.

## Signals
- Embedded: `send_signal` writes into local `pending_signals`/`custom_signals` tables; instance picks up on next poll/checkpoint.
- Server: `send_signal` uses management RPC over QUIC.
- Hybrid: environment proxies signals to the embedded core’s management listener; delivery is still via local store to the instance.

## Storage
- SQLite/libsql adapter with WAL enabled for concurrency; migrations adjusted for SQLite types (TEXT/BLOB/CHECK, `CURRENT_TIMESTAMP`).
- Postgres adapter stays as-is; same logical schema.

## Hybrid networking notes
- Core keeps instance + management listeners up. Environment points to that management addr; instances in containers point to the instance addr (via host networking/port mapping or libsql URL if used).
- Durability stays local; environment doesn’t see the storage backend.

## Risks / follow-ups
- SQLite concurrency at higher load; consider libsql or Postgres if many parallel instances.
- Registration/heartbeat protocol with environment needs to be defined and versioned.
- If durable sleep returns, add a small host-side wake loop in embedded mode; until then, in-process sleep only.
