# runtara-core

[![License](https://img.shields.io/crates/l/runtara-core.svg)](LICENSE)

Durable execution engine for [Runtara](https://runtara.com). Manages checkpoints, signals, and instance events with persistence to PostgreSQL or SQLite.

## Overview

This crate is the heart of the Runtara platform, providing:

- **Checkpoint Persistence**: Save and restore workflow state for crash recovery
- **Signal Delivery**: Cancel, pause, and resume signals to running instances
- **Durable Sleep**: Store wake timestamps for long-running sleeps
- **Instance Events**: Track heartbeats, completion, failure, and suspension
- **QUIC Transport**: Fast, secure communication with workflow instances

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         External Clients                                 │
│                    (runtara-management-sdk, CLI)                         │
└─────────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                      runtara-environment                                 │
│            (Image Registry, Instance Lifecycle, Wake Queue)              │
│                           Port 8002                                      │
└─────────────────────────────────────────────────────────────────────────┘
          │                                              │
          │ Shared Persistence                           │ Spawns
          ▼                                              ▼
┌───────────────────────┐                    ┌─────────────────────────────┐
│    runtara-core       │◄───────────────────│     Workflow Instances      │
│    (This Crate)       │  Instance Protocol │   (using runtara-sdk)       │
│  Checkpoints/Signals  │                    │                             │
│  Port 8001            │                    └─────────────────────────────┘
└───────────────────────┘
          │
          ▼
┌───────────────────────┐
│  PostgreSQL / SQLite  │
│  (Durable Storage)    │
└───────────────────────┘
```

## Running the Server

```bash
# Set required environment variables
export RUNTARA_DATABASE_URL=postgres://user:pass@localhost/runtara

# Run the server
cargo run -p runtara-core
```

## Instance Protocol (Port 8001)

Workflow instances connect via QUIC using `runtara-sdk`. The protocol supports:

| Operation | Description |
|-----------|-------------|
| `RegisterInstance` | Self-register on startup, optionally resume from checkpoint |
| `Checkpoint` | Save state (or return existing if checkpoint_id exists) + signal delivery |
| `GetCheckpoint` | Read-only checkpoint lookup |
| `Sleep` | Durable sleep - stores wake time in database |
| `InstanceEvent` | Fire-and-forget events (heartbeat, completed, failed, suspended) |
| `GetInstanceStatus` | Query instance status |
| `PollSignals` | Poll for pending cancel/pause/resume signals |
| `SignalAck` | Acknowledge receipt of a signal |

### Checkpoint Semantics

The `Checkpoint` operation is the primary durability mechanism:

1. **First call with checkpoint_id**: Saves state, returns empty `existing_state`
2. **Subsequent calls with same checkpoint_id**: Returns existing state (for resume)
3. **Signal delivery**: Returns pending signals in response for efficient poll-free detection

### Durable Sleep

The `Sleep` operation stores a `sleep_until` timestamp in the instances table.
Environment's wake scheduler polls for sleeping instances and relaunches them
when their wake time arrives.

## Instance Status State Machine

```
                    ┌─────────┐
                    │ PENDING │
                    └────┬────┘
                         │ register
                         ▼
                    ┌─────────┐
         ┌──────────│ RUNNING │──────────┐
         │          └────┬────┘          │
         │               │               │
    pause│          sleep│          cancel
         │               │               │
         ▼               ▼               ▼
    ┌──────────┐   ┌──────────┐   ┌───────────┐
    │SUSPENDED │   │SUSPENDED │   │ CANCELLED │
    └────┬─────┘   └────┬─────┘   └───────────┘
         │               │
    resume│          wake│
         │               │
         └───────┬───────┘
                 │
                 ▼
            ┌─────────┐
            │ RUNNING │──────────┬──────────┐
            └─────────┘          │          │
                            complete      fail
                                 │          │
                                 ▼          ▼
                           ┌───────────┐ ┌────────┐
                           │ COMPLETED │ │ FAILED │
                           └───────────┘ └────────┘
```

| Status | Description |
|--------|-------------|
| `PENDING` | Instance created but not yet registered |
| `RUNNING` | Instance is actively executing |
| `SUSPENDED` | Instance paused (by signal) or sleeping (durable sleep) |
| `COMPLETED` | Instance finished successfully |
| `FAILED` | Instance failed with error |
| `CANCELLED` | Instance was cancelled via signal |

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_DATABASE_URL` | Yes | - | PostgreSQL or SQLite connection string |
| `RUNTARA_QUIC_PORT` | No | `8001` | Instance QUIC server port |
| `RUNTARA_MAX_CONCURRENT_INSTANCES` | No | `32` | Maximum concurrent instances |

## Database

Core uses SQLx with support for both PostgreSQL and SQLite:

- PostgreSQL: Recommended for production
- SQLite: Suitable for development and testing

Migrations are in `migrations/postgresql/` and `migrations/sqlite/`.

### Running Migrations

Migrations run automatically on server startup. For manual migration:

```bash
# PostgreSQL
sqlx migrate run --source crates/runtara-core/migrations/postgresql

# SQLite
sqlx migrate run --source crates/runtara-core/migrations/sqlite
```

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `server` | Yes | Full server mode with QUIC transport |

Without the `server` feature, only the persistence layer is available (used by `runtara-environment` for shared database access).

## Testing

```bash
# Run unit tests
cargo test -p runtara-core

# Run with database (integration tests)
TEST_DATABASE_URL=postgres://... cargo test -p runtara-core
```

## Related Crates

- [`runtara-sdk`](https://crates.io/crates/runtara-sdk) - Client SDK for workflow instances
- [`runtara-environment`](../runtara-environment) - Instance lifecycle and OCI container management
- [`runtara-protocol`](https://crates.io/crates/runtara-protocol) - Wire protocol definitions

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
