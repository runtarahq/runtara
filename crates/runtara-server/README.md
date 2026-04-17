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
accept scenario definitions, compile them, execute instances, and observe
results lives here. The public surface is the HTTP API — the crate also exposes
a thin library (`runtara_server::start`) plus a few re-exports for host
applications that want to embed the server inside a larger binary.

## Using it standalone

Run it directly from the workspace:

```bash
cargo run -p runtara-server --release
```

The binary reads `.env` via `dotenvy`, then requires a PostgreSQL connection
string in either `DATABASE_URL` or `OBJECT_MODEL_DATABASE_URL`. On start it
runs the SQL migrations shipped under `crates/runtara-server/migrations`; set
`SKIP_MIGRATIONS=true` to bypass on pre-migrated databases. All other knobs —
bind address, Valkey URL, JWT secrets, OTLP endpoint, worker concurrency — are
plain environment variables read by `config.rs`; see that module for the
authoritative list. Once the server is up, the OpenAPI spec is exposed by the
router and the MCP transport is mounted under the `mcp` module's routes.

## Inside Runtara

- Depends on `runtara-workflows`, `runtara-core` (with the `server` feature),
  and `runtara-environment` for execution, persistence, and the object model.
- Links `runtara-agents` with `integrations` + `native` features and
  re-exports `runtara_agents::integrations` so the `inventory`-based agent
  registry survives linker optimization.
- Pulls in `runtara-management-sdk`, `runtara-dsl`, `runtara-connections`,
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
