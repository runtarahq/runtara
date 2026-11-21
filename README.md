# Runtara

**Secure AI automation from workflow design to live operations.**

Runtara is a platform for building, running, and operating AI workflows as
production systems. It brings workflows, triggers, connections, operational
data, conversations, and durable execution together behind one API, with
every workflow step running in a WebAssembly sandbox and every credential
kept sealed.

[Website](https://runtara.com) | [Product docs](https://runtara.com/docs/runtara-platform) 

This repository is the server and runtime. The web console lives in
[runtarahq/runtara-frontend](https://github.com/runtarahq/runtara-frontend)
and is deployed on its own; the Docker Compose stack below runs both.

## Try It With Docker Compose

```bash
curl -fsSL https://raw.githubusercontent.com/runtarahq/runtara/refs/heads/main/docker-compose.yml \
    -o docker-compose.yml \
    && FETCH_EXAMPLES=yes RUNTARA_PORT=3030 docker compose up
```

Open **http://localhost:3030/ui/**. The stack starts the server and console
images plus PostgreSQL and Valkey, wired for password-free local auth so there
is no identity provider to configure. `FETCH_EXAMPLES=yes` seeds the example
workflows from [`examples/`](examples/README.md) (conditionals, error
handling, split, while loops, an HTTP call).

Without `RUNTARA_PORT` the console is on port 3000; the API is on 7001
(`RUNTARA_API_PORT`). This is a turnkey trial stack, not a production
deployment.

## What Runtara Does

- **Workflows**: JSON execution graphs of agent steps, conditionals, splits,
  loops, waits, child workflows, and error paths, validated and compiled to
  WebAssembly components. Workflows are versioned and can be called as tools
  by AI agents.
- **Triggers**: start workflows from HTTP calls, cron schedules, incoming
  email, and events from connected applications.
- **Integrations**: 39 agent components, from HTTP, CSV, XML, XLSX, crypto,
  and transforms to OpenAI, Bedrock, MCP, Shopify, Stripe, HubSpot, Slack,
  Jira, QuickBooks, Xero, Zendesk, S3, Azure Blob Storage, SharePoint, and
  more (`crates/agents`).
- **Connections**: provider credentials, OAuth, and rate limits, managed
  outside workflow definitions and stored sealed.
- **Object model**: schema-driven operational records stored in a tenant's own
  PostgreSQL database, reachable from workflows and the API.
- **Conversations**: chat with a workflow. A conversation is found by a key,
  kept by the engine, and carried by one run after another; chat platforms
  connect to it as integrations built from workflows (see
  [`examples/integrations`](examples/integrations)).
- **Operations**: views and processes for running and monitoring work across
  workflows.
- **Executions and analytics**: execution history, step events, metrics,
  usage, and rate-limit visibility.
- **MCP**: an MCP endpoint (`/mcp`) for running and authoring workflows from
  AI clients.

## Security And Runtime Design

- **Sandboxed steps**: agents and workflows are WebAssembly components with no
  raw network, filesystem, or credential access. Runs can also be placed on
  executors, each run in a sandboxed process of its own
  (`RUNTARA_RUNNER=executor`).
- **Sealed credentials**: a connection's secrets are sealed by
  [seal](https://github.com/runtarahq/seal), which attaches them to outgoing
  requests. Nothing else reads them, and seal can run as a separate process
  (`RUNTARA_SEAL_MODE`).
- **Durable execution**: runs checkpoint, park on waits and signals, resume
  after crashes, and do not replay completed work.
- **Tenant isolation**: every database access runs in a tenant's scope and is
  enforced by PostgreSQL row-level security.
- **Separable roles**: one `runtara-server` binary runs as the API, the
  engine, the intake for public webhooks, or all three (`RUNTARA_ROLE`). Split
  apart, the API holds no runtime state or keys and asks the engine over
  mutual TLS.

```text
                ┌─────────────┐
 webhooks,      │   intake    │  public routes only; no run state, no keys
 OAuth ───────▶ └──────┬──────┘
 redirects             │ mTLS
                ┌──────▼──────┐        ┌──────────┐
 console, API,  │   engine    │◀──────▶│   seal   │  holds connection secrets
 MCP ──▶ api ──▶│ core + env  │        └──────────┘
          mTLS  └──────┬──────┘
                       │ mTLS
                ┌──────▼──────┐
                │  executors  │  one sandboxed process per run (optional)
                └─────────────┘
```

The Compose overlays bring up each topology:

| Overlay | Adds |
| --- | --- |
| `docker-compose.seal.yml` | seal as its own process |
| `docker-compose.executor.yml` | executors; runs and compiles leave the server |
| `docker-compose.roles.yml` | API, engine, and intake as separate processes (needs the seal overlay) |
| `docker-compose.engines.yml` | several engine replicas (on top of the split stack) |

Each file's header comment gives the exact `docker compose -f ...` command.

## Running From Source

### Prerequisites

- Rust, pinned by `rust-toolchain.toml` (it also installs the
  `wasm32-wasip2` target).
- PostgreSQL for server and runtime state.
- Valkey (or Redis).

`dev/README.md` describes a Docker Compose stack for these dependencies.

### Start The Server

```bash
./start.sh            # ./start.sh help lists the settings
```

`start.sh` builds and runs `runtara-server`, generating a credential
encryption key once into `.data/credential-encryption.key`. To run the server
directly:

```bash
export RUNTARA_DATABASE_URL=postgres://localhost/runtara
export VALKEY_HOST=localhost
# Keep this key: what is stored under a lost key cannot be recovered.
export RUNTARA_CONNECTIONS_ENCRYPTION_KEY="$(openssl rand -base64 32)"
cargo run -p runtara-server --release
```

The API listens on port `7001`. Valkey supports auth (`VALKEY_USER` /
`VALKEY_PASSWORD`) and TLS (`VALKEY_TLS=1`, with `VALKEY_TLS_CA_CERT` for a
self-signed certificate).

To give a tenant an object model, enter its PostgreSQL connection with
`runtara-server object-model set`; it is stored sealed.

### Images And Releases

Each release publishes a multi-arch server image, `ghcr.io/runtarahq/runtara`
(tags `:X.Y.Z`, `:latest`, and a rolling `:dev`), and a native bundle:

```bash
curl -fsSL https://github.com/runtarahq/runtara/releases/latest/download/install.sh | sh
```

The image holds `runtara-server` with its toolchain and agent components;
supply PostgreSQL, Valkey, and configuration through the environment, using
`docker-compose.yml` as the reference. State is written under `/data`. Run
the console image (`ghcr.io/runtarahq/runtara-frontend`) beside it with
`RUNTARA_UI_API_BASE_URL` pointing at the server, and list the console's
origin in the server's `CORS_ALLOWED_ORIGINS` and `RUNTARA_UI_PUBLIC_URL`.
The native install is headless: the API and MCP only.

## Build And Test

```bash
cargo build
cargo test                        # runtara-server only (the default member)
cargo test -p runtara-workflows   # one crate
scripts/build-agent-components.sh # agent .wasm components
./e2e/run_all.sh                  # a selected end-to-end suite
```

`.github/workflows/ci.yml` is the authoritative build and lint matrix; some
integration tests need feature flags and test databases.

## Workflow DSL

Workflows are JSON execution graphs. A minimal one:

```json
{
  "name": "Simple Passthrough",
  "description": "A simple workflow that passes input directly to output",
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "result": {
          "valueType": "reference",
          "value": "data.input"
        }
      }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}
```

The live JSON Schema is served at `GET /api/runtime/specs/dsl`. See also
`crates/runtara-dsl/README.md`, `crates/runtara-workflows/README.md`, and the
workflows in `examples/workflows`.

## Repository Layout

| Path | Contents |
| --- | --- |
| `crates/runtara-server` | The server: HTTP API, auth, MCP, workers, OpenAPI; runs as API, engine, intake, or all |
| `crates/runtara-core` | Durable execution: instances, checkpoints, signals, events, sleep |
| `crates/runtara-environment` | Image and instance lifecycle, runners, wake scheduling |
| `crates/runtara-run`, `runtara-executor`, `runtara-runner-executor` | Running workflows outside the server, one sandboxed process per run |
| `crates/runtara-dsl` | Workflow schema and step metadata |
| `crates/runtara-workflows` | Workflow validation and compilation |
| `crates/runtara-workflow-stdlib` | Guest-side workflow runtime |
| `crates/runtara-component-host` | Loading and invoking WebAssembly components |
| `crates/runtara-wit` | All WIT contracts |
| `crates/agents/runtara-agent-*` | Workflow agents, one component crate each |
| `crates/runtara-agents` | Host-side agent support (connection extractors) |
| `crates/runtara-connections` | Connections, OAuth, sealed credentials, rate limiting |
| `crates/runtara-object-store` | Schema-driven object storage |
| `crates/runtara-tenant-scope` | Tenant scoping for database work |
| `crates/runtara-validation-wasm` | Workflow validation for the browser console |
| `examples` | Example workflows and the seeding script |
| `e2e` | End-to-end tests |
| `dev` | Development dependencies stack |
| `scripts` | Install, build, and role-provisioning scripts |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [CLA.md](CLA.md), and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Report vulnerabilities as described
in [SECURITY.md](SECURITY.md).

## License

Licensed under `AGPL-3.0-or-later`; see [LICENSING.md](LICENSING.md) for
details and commercial licensing. Contact `hello@syncmyorders.com`.

Copyright (C) 2025 SyncMyOrders Sp. z o.o.
