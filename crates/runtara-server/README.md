# runtara-server

The all-in-one HTTP API server binary for Runtara — one process, one port, the whole platform.

## What it is

`runtara-server` is the native host binary that fronts a full Runtara deployment
over HTTP. It wires together the workflow engine, agent integrations, the DSL
compiler, the management SDK, the object-model repositories, and a
Valkey/Redis-backed channel system behind a single `axum` router with an
`utoipa`-generated OpenAPI document.

The server also embeds an `rmcp`-based MCP endpoint, an OpenTelemetry exporter
(traces, metrics, logs via OTLP), and the background workers that drive
scheduling, dispatch, and agent-test execution. Everything an operator needs to
accept workflow definitions, compile them, execute instances, and observe
results lives here. The public surface is the HTTP API — the crate also exposes
a thin library (`runtara_server::start`) plus a few re-exports for host
applications that want to embed the server inside a larger binary.

## Using it standalone

Run it directly from the workspace:

```bash
cargo run -p runtara-server --release
```

The binary reads `.env` via `dotenvy`, then requires a PostgreSQL connection
string in `RUNTARA_SERVER_DATABASE_URL`. On start it
runs the SQL migrations shipped under `crates/runtara-server/migrations`; set
`SKIP_MIGRATIONS=true` to bypass on pre-migrated databases. All other knobs —
bind address, Valkey URL, JWT secrets, OTLP endpoint, worker concurrency — are
plain environment variables read by `config.rs`; see that module for the
authoritative list. Once the server is up, the OpenAPI spec is exposed by the
router and the MCP transport is mounted under the `mcp` module's routes.

`RUNTARA_RUNTIME_POLL_TIMEOUT_SECS` (default `300`) bounds how long the server
waits for a workflow instance to reach a terminal state when the caller names no
timeout of its own. It replaces `RUNTARA_REQUEST_TIMEOUT_MS`, which is the
runtara-sdk per-request HTTP timeout and never meant this: an operator who
raised it for the SDK also moved this wait, and one who set it here was writing
milliseconds for a value kept in seconds. The old name is still read when the
new one is unset — a deployment that relied on it keeps the same wait — and the
server logs a warning naming both the milliseconds it found and the seconds it
derived. Values under `1000` truncate to a zero-second wait, so migrate rather
than leave the old name in place.

## Embedded UI (optional)

The crate can bundle the `./frontend` React app into the binary behind the
`embed-ui` cargo feature:

```bash
(cd frontend && npm ci && npm run build)      # produces frontend/dist/
cargo build -p runtara-server --features embed-ui
```

`npm run build` refreshes the browser validation WASM package before
creating `frontend/dist`. The `embed-ui` Cargo build also refreshes it when
Rust validation or agent metadata sources change, then rebuilds `frontend/dist`
so embedded assets stay in sync. Install `wasm-pack` if the generated package
is stale:

```bash
cargo install wasm-pack --locked
```

Without env config the UI is served at `/ui/` (self-hosted). For a tenant
deployed behind a gateway that routes `/ui/<tenant-id>/…` externally, set
`RUNTARA_UI_BASE_PATH=/ui/<tenant-id>` so the `<base href>` injected into
`index.html` points the browser at tenant-scoped asset URLs. The Axum mount
prefix stays at `/ui` (override via `RUNTARA_UI_MOUNT` only if the gateway
does not strip the tenant segment before forwarding).

`RUNTARA_UI_DIST_DIR=<path to frontend/dist>` serves the UI from that directory
at request time instead of from the embedded snapshot. It works with or without
`embed-ui` and in any profile, and takes precedence when both are available.
Its point is development: an embedded bundle is fixed at link time, so
`npm run build:watch` output is invisible to a `--release` server until the
binary is rebuilt. Pointing at `dist` makes a save plus a reload the whole loop
— and lets a dev build drop `embed-ui`, which is what stops a running watcher
from invalidating every cargo build. Assets from a disk source are served
`no-cache` rather than `immutable`, and a read that lands mid-rebuild returns
503 rather than a stale page. See `frontend/README.md` for the dev loops.

## Inside Runtara

- Depends on `runtara-workflows`, `runtara-core`, and `runtara-environment` for
  execution, persistence, and the object model. Core is transport-free, so this
  crate owns the instance protocol's HTTP surface in `core_runtime`.
- Links `runtara-agents` with `integrations` + `native` features and
  re-exports `runtara_agents::integrations` so the static agent registry keeps
  integration modules reachable.
- Pulls in `runtara-dsl`, `runtara-connections`,
  `runtara-object-store`, `runtara-text-parser`, and `runtara-workflow-stdlib`
  to expose their functionality over the HTTP API.
- The main integration point is the external REST + MCP surface: `axum`
  handlers under `src/api/`, MCP transport under `src/mcp/`, and the generated
  OpenAPI document served by `server::start`.
- Runs as a native host binary — not a WASM target — because it owns the
  Postgres pool, background workers, and OTLP exporter.
- No workspace crate depends on `runtara-server`; it sits at the top of the
  dependency graph.

## License

AGPL-3.0-or-later.
