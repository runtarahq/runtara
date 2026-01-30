# runtara-environment

[![License](https://img.shields.io/crates/l/runtara-environment.svg)](LICENSE)

Instance lifecycle management for [Runtara](https://runtara.com). Handles image registration, OCI container execution, and wake scheduling for durable workflows.

## Overview

This crate is the control plane for the Runtara platform, providing:

- **Image Registry**: Store and manage compiled workflow binaries
- **Instance Lifecycle**: Start, stop, and monitor workflow instances
- **OCI Container Runner**: Execute workflows in isolated containers via crun
- **Wake Scheduler**: Relaunch sleeping instances when their wake time arrives
- **Signal Proxying**: Forward cancel/pause/resume signals to runtara-core

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         External Clients                                 │
│                    (runtara-management-sdk, CLI)                         │
└─────────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                   runtara-environment (This Crate)                       │
│                         Port 8002                                        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐     │
│  │   Image     │  │  Instance   │  │    Wake     │  │  Container  │     │
│  │  Registry   │  │  Lifecycle  │  │  Scheduler  │  │   Runner    │     │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘     │
└─────────────────────────────────────────────────────────────────────────┘
          │                 │                              │
          │                 │ Proxy signals                │ Spawn
          │                 ▼                              ▼
          │       ┌───────────────────┐        ┌─────────────────────────┐
          │       │   runtara-core    │◄───────│   Workflow Instances    │
          │       │   Port 8001       │        │   (OCI containers)      │
          │       └───────────────────┘        └─────────────────────────┘
          │                 │
          ▼                 ▼
┌───────────────────────────────────────────────────────────────────────┐
│                           PostgreSQL                                   │
│              (Images, Instances, Checkpoints, Events)                  │
└───────────────────────────────────────────────────────────────────────┘
```

## Running the Server

```bash
# Set required environment variables
export RUNTARA_DATABASE_URL=postgres://user:pass@localhost/runtara

# Run the server
cargo run -p runtara-environment
```

## Environment Protocol (Port 8002)

External clients connect via QUIC using `runtara-management-sdk`. The protocol supports:

### Image Operations

| Operation | Description |
|-----------|-------------|
| `RegisterImage` | Register a new image (single-frame upload < 16MB) |
| `RegisterImageStream` | Register a large image via streaming upload |
| `ListImages` | List images with optional tenant filter and pagination |
| `GetImage` | Get image details by ID |
| `DeleteImage` | Delete an image |

### Instance Operations

| Operation | Description |
|-----------|-------------|
| `StartInstance` | Start a new instance from an image |
| `StopInstance` | Stop a running instance with grace period |
| `ResumeInstance` | Resume a suspended instance |
| `GetInstanceStatus` | Query instance status |
| `ListInstances` | List instances with filtering and pagination |

### Signal Operations

| Operation | Description |
|-----------|-------------|
| `SendSignal` | Send cancel/pause/resume signal to instance |

Signals are proxied to runtara-core which stores them for the instance to poll.

## Runner Types

Environment supports multiple execution backends:

| Runner | Description |
|--------|-------------|
| OCI (default) | Execute in OCI containers via crun |
| Mock | In-memory testing runner |

### OCI Runner

The OCI runner:
1. Creates OCI bundles from registered images
2. Launches containers with crun
3. Mounts instance I/O directories for input/output exchange
4. Monitors container lifecycle and collects metrics

Instance I/O is exchanged via files:
- Input: `{DATA_DIR}/{tenant_id}/runs/{instance_id}/input.json`
- Output: `{DATA_DIR}/{tenant_id}/runs/{instance_id}/output.json`

## Wake Scheduler

The wake scheduler handles durable sleep:

1. Polls database for instances with `sleep_until` in the past
2. Relaunches the container for each waking instance
3. The SDK inside the container calculates remaining sleep time

This enables workflows to sleep for hours/days without holding resources.

## Configuration

### Core Settings

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `RUNTARA_DATABASE_URL` | Yes | - | PostgreSQL connection string |
| `RUNTARA_ENV_QUIC_PORT` | No | `8002` | Environment QUIC server port |
| `RUNTARA_CORE_ADDR` | No | `127.0.0.1:8001` | Address of runtara-core. For pasta networking, use host's actual IP |
| `DATA_DIR` | No | `.data` | Data directory for images and bundles |
| `RUNTARA_SKIP_CERT_VERIFICATION` | No | `false` | Skip TLS verification (passed to instances) |

### OCI Runner Settings

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `BUNDLES_DIR` | No | `${DATA_DIR}/bundles` | Directory for OCI bundles |
| `EXECUTION_TIMEOUT_SECS` | No | `300` | Default execution timeout in seconds |
| `USE_SYSTEMD_CGROUP` | No | `false` | Use systemd for cgroup management |

## Database

Environment shares the database with runtara-core. It manages:

- `instance_images`: Maps instances to their source images
- Image storage and metadata

Migrations are in `migrations/`.

### Running Migrations

Migrations run automatically on server startup. For manual migration:

```bash
sqlx migrate run --source crates/runtara-environment/migrations
```

## Testing

```bash
# Run unit tests
cargo test -p runtara-environment

# Run with database (integration tests)
TEST_DATABASE_URL=postgres://... cargo test -p runtara-environment
```

## Modules

| Module | Description |
|--------|-------------|
| `config` | Server configuration from environment variables |
| `db` | PostgreSQL persistence for images and instances |
| `handlers` | Environment protocol request handlers |
| `image_registry` | Image storage and retrieval |
| `container_registry` | Running container tracking |
| `instance_output` | Reading output.json from completed instances |
| `runner` | Container/process execution backends |
| `server` | QUIC server implementation |
| `wake_scheduler` | Durable sleep wake scheduling |

## Related Crates

- [`runtara-core`](../runtara-core) - Checkpoint and signal persistence
- [`runtara-management-sdk`](https://crates.io/crates/runtara-management-sdk) - Client SDK for management operations
- [`runtara-protocol`](https://crates.io/crates/runtara-protocol) - Wire protocol definitions
- [`runtara-dsl`](https://crates.io/crates/runtara-dsl) - DSL types for agent metadata

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
