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

`RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS` (default `300`) is the execution timeout
applied to a workflow instance whose definition names no `executionTimeoutSeconds`
of its own. It is a kill deadline, not a patience setting: runtara-environment's
container monitor stops the instance when it elapses and records the run `failed`
with `termination_reason: timeout`, and a synchronous execution cancels it. It is
read by `runtime_client.rs`, not by `config.rs`. The server always sends a
timeout, so it is this value rather than runtara-environment's own
`RUNTARA_DEFAULT_INSTANCE_TIMEOUT_SECS` (3600) that governs anything the server
starts.

It replaces `RUNTARA_REQUEST_TIMEOUT_MS`, which was the runtara-sdk per-request
HTTP timeout, in milliseconds, and never meant this. The two never collided in
effect — a server process does not run the SDK, and `build_env` does not forward
the variable to workflow guests — but they collided in name and in unit: an
operator who set it here was writing milliseconds for a value the server keeps in
seconds, and divided it by 1000 to get there.

The old name is still read when the new one is unset, so a deployment that relied
on it keeps the timeout it had, and the server warns, quoting both the
milliseconds it found and the seconds it derived. A value under `1000` truncates
to zero seconds, under which every workflow is killed the instant it starts —
migrate rather than leave the old name in place. Whichever name supplies it, the
resolved timeout is logged at startup.

## Console

The server serves no UI. The console lives in
[runtarahq/runtara-frontend](https://github.com/runtarahq/runtara-frontend)
and is deployed on its own — its image `ghcr.io/runtarahq/runtara-frontend`
or a static host — calling this server's API from another origin. Console X.Y
works with server X.Y and the minor on either side.

The server needs two settings for it:

- `CORS_ALLOWED_ORIGINS` — the console's origin, e.g.
  `https://app.example.com`, so the browser lets it call the API.
- `RUNTARA_UI_PUBLIC_URL` — where the console is served, e.g.
  `https://app.example.com/`. The OAuth callback sends the consent popup to
  the console's `oauth-complete.html` there; without it the callback answers
  in plain text.

The console's own settings (auth mode, OIDC client, API URL) are the
`RUNTARA_UI_*` variables of its image; see runtara-frontend's README.

## Inside Runtara

- Depends on `runtara-workflows`, `runtara-core`, and `runtara-environment` for
  execution, persistence, and the object model. Core is transport-free, so this
  crate owns the instance protocol's HTTP surface in `core_runtime`: a test
  and debugging transport, served on loopback only when
  `RUNTARA_CORE_INSTANCE_API` is enabled. Workflows reach Core in-process.
- Links `runtara-agents` for host connection support and the static metadata
  registry. Workflow capabilities execute in standalone WASM components,
  including [Slack and Mailgun attachment downloads](../../docs/workflow-attachments.md).
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
