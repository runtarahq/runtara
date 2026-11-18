# Runtara Agent Guide

This file is the shared source of truth for coding assistants working in this
repository. Prefer current code and configuration over older prose. When
sources disagree, use this order: implementation, generated contracts, CI,
scoped README files, examples.

## Architecture

- `crates/runtara-server` is the native application and composition root. It
  owns the public/internal HTTP APIs, auth, MCP endpoint, workers, and OpenAPI.
  It serves no UI. It runs as its API, its engine, or both (`RUNTARA_ROLE`,
  `docs/roles.md`): the API role holds nothing of the engine's and asks an
  engine in another process over mutual TLS (`src/engine/`).
- `crates/runtara-core` owns durable execution state: instances, checkpoints,
  signals, events, and sleep.
- `crates/runtara-environment` owns image and instance lifecycle, runners, and
  wake scheduling. It is embedded by the server.
- `crates/runtara-run`, `crates/runtara-executor` and
  `crates/runtara-runner-executor` run workflows outside the server
  (`RUNTARA_RUNNER=executor`, `docs/executor.md`): one sandboxed process per
  run, the executor service that starts them, and the server's side of it.
  `runtara-run` holds a guest and nothing else: give it no credential, no
  address and no dependency it does not need.
- `crates/runtara-dsl` defines workflow schemas and static step metadata.
- `crates/runtara-workflows` validates and compiles workflow graphs.
- `crates/runtara-workflow-stdlib` provides guest-side workflow execution.
- `crates/runtara-wit` holds every runtara WIT contract, grouped into packages
  by who may import them, plus the interface-name constants and the per-agent
  package generator. Do not add WIT anywhere else.
- `crates/runtara-component-host` loads and invokes WASM components.
- Workflow agents are standalone component crates under
  `crates/agents/runtara-agent-*`. To add or change an integration agent or a
  connection type, follow `docs/adding-an-agent.md`.
- `crates/runtara-agents` contains host-only support, including connection
  extractors. Do not add a
  normal workflow agent there.
- `crates/runtara-connections` owns credential storage, OAuth, connection
  schemas, and rate limiting. A connection's secrets are stored and used
  sealed only (its `sealed` module, over
  [seal](https://github.com/runtarahq/seal)): seal attaches them to the
  requests it sends, and nothing else reads them. seal runs in the server
  or as a process of its own (`RUNTARA_SEAL_MODE`, `docs/seal.md`); code
  reaches it through `SealedCredentials` either way, never through the keys.
- `crates/runtara-object-store` owns schema-driven tenant object storage.
- The console (React/Vite) lives in
  [runtarahq/runtara-frontend](https://github.com/runtarahq/runtara-frontend)
  and is deployed on its own (its image `ghcr.io/runtarahq/runtara-frontend`,
  or a static host), calling the server's API across origins. Console changes
  go to that repository. The release smoke test runs the console release named
  by `RUNTARA_FRONTEND_VERSION` in `.github/workflows/release.yml`.
- `prototypes` and `spikes` are experimental and are not production code unless
  a task explicitly targets them.

## Toolchains

- Rust is pinned by `rust-toolchain.toml`; do not substitute an older toolchain.
- Agent and workflow components target `wasm32-wasip2` and build with plain
  Cargo through `scripts/build-agent-components.sh`. Do not introduce
  `cargo-component` or committed generated `bindings.rs` files.

## Working Rules

- Make the smallest correct change and follow patterns in the affected module.
- Search for every exhaustive match or registry when adding DSL variants,
  metadata fields, error types, or connection types.
- Keep host and WASM boundaries explicit. WASM agents do not receive raw
  network, filesystem, or credential access. No component is given a
  connection's credentials, the built-in `trusted` capabilities included:
  what one would do with a key (presigning a URL), the host has seal do.
- Do not add a path that stores a connection secret readable, or that uses
  one except through seal. A new connection type says where each secret goes
  in `runtara-connections`' `sealed::policy`; a secret seal cannot hold is
  refused when the connection is saved. The same holds for a platform OAuth
  app's client secret: it is sealed as it is entered (`runtara-server
  oauth-apps put`) and kept in `platform_oauth_app`; no file, setting or
  host hands one over readable.
- The platform supplies no object model database. A tenant's is a `postgres`
  connection of the tenant's, entered with `runtara-server object-model set`
  and sealed; do not add a database URL to the server's configuration for
  tenants' data.
- Work that reaches the server or runtime database runs in a scope
  (`runtara-tenant-scope`): a tenant's (`scoped`), or the platform's own
  across tenants (`system`), and only what is no one tenant's is the
  platform's. Work in neither is refused its rows by row-level security.
  A spawned task leaves the scope: spawn with `tenant_scope::spawn`, or wrap
  the work in `inherit`. The test databases connect as a superuser, which
  row-level security does not bind, so CI does not catch a missing scope;
  `e2e/test_served_tenants.sh` runs the server as a plain role and does.
- Code that takes requests (`runtara-server`'s `api/handlers`, `mcp`) reaches
  the engine through `Engine` (`src/engine.rs`) and nothing else: no
  `RuntimeClient`, no `ExecutionEngine`, no connections facade or seal, no
  component dispatcher, no compile queue. A new operation is a method there,
  naming its tenant; a route whose answer uses a connection's credentials is
  served by the engine (`Engine::serve`). Background work is started through
  `roles::Workers`, for the role it belongs to. See
  `docs/separating-concerns.md`.
- An operation of `Engine` is also a message in `src/engine/wire.rs` and a
  method of `RemoteEngine`, and what it speaks is serializable: the API may
  be another process. The API's side (`api_app` and `start_as_api` in
  `server.rs`, `api/handlers`, `mcp`) reads nothing the engine set in its own
  process: no artifact store, no staged agents, no installed component
  release. What it needs of those it asks the engine. Do not add to what the
  API role is given (`roles::given_to_the_api_role` lists what it refuses).
- The engine takes nothing on the API's word. A call made for a caller
  crosses with what the caller presented (`auth::presented`), and the
  engine verifies it (`engine/listener.rs`): work that outlives its request
  keeps it with `presented::inherit` or `presented::kept`, as it keeps the
  tenant's scope. An operation that changes something names the permissions
  it `needs` in `wire.rs`. What makes a workflow runnable is the engine's
  record in the runtime database (`engine/approvals.rs`), not a row of the
  server database: do not add a path that runs a definition, an image or a
  version because the server database names it. The API's side does not
  write a table the engine's records are in, nor look across tenants; a new
  table the API reads or writes is granted in
  `scripts/roles/api-database-role.sql`, and a new Valkey key in
  `scripts/roles/api-valkey-user.acl`.
- Never read, print, log, or commit credential values. Treat `.env*`, tokens,
  connection parameters, and database URLs as sensitive.
- Do not hand-edit generated artifacts or ignored build outputs.
- Do not modify an already committed SQL migration. Add a forward migration.
- Do not kill processes broadly, drop databases, remove caches, rewrite Git
  history, commit, tag, or push unless the user explicitly requests it.
- Preserve unrelated worktree changes.

## Generated Files

- Each release attaches `openapi.json` (from `dump_openapi`) and the browser
  validation WASM package (`runtara-validation-wasm-<version>.tgz`, from
  `crates/runtara-validation-wasm`) for runtara-frontend, which builds against
  them (`.github/workflows/frontend-inputs.yml`).
- Agent `.wasm` and `.meta.json` files are produced under
  `target/wasm32-wasip2/release` by
  `scripts/build-agent-components.sh`.
- Generated agents: `crates/agents/*/src/generated.rs`, `spec.slice.json` and
  the fetched `tests/fixtures/*.json` are produced by `scripts/connector_gen`
  from the crate's `connector.yaml`. Change `connector.yaml` and regenerate;
  changing a generated file any other way, or the templates in
  `scripts/connector_gen/templates/`, needs explicit approval from the user.
- SQLx offline metadata under `crates/runtara-server/.sqlx` is generated by
  `cargo sqlx prepare`; update it only when compile-time checked queries change.
- WIT dependencies under `crates/runtara-wit/wit/deps` are committed
  resolved inputs. Update them through the WIT dependency tooling, not by hand.

## Verification

Start with the narrowest relevant checks, then widen based on the changed
boundary. A bare `cargo test` covers only the workspace default member, not the
whole workspace.

- Rust formatting: `cargo fmt --all -- --check`.
- Focused Rust tests: `cargo test -p <crate>`.
- Focused Rust lint: `cargo clippy -p <crate> --all-targets -- -D warnings`.
- The complete feature-gated lint/build matrix is authoritative in
  `.github/workflows/ci.yml`; integration test targets can be silently omitted
  without their required feature.
- Component changes: run `scripts/build-agent-components.sh`, then the relevant
  `runtara-component-host` and `runtara-workflows` integration tests from CI.
- Validation WASM changes (`runtara-validation-wasm` and the crates it uses):
  build it with `wasm-pack build crates/runtara-validation-wasm --target web`.
- Database changes: run the owning crate's feature-gated integration tests
  against isolated test databases.
- Use top-level `e2e/test_*.sh` scripts only when their external boundary is
  relevant. `e2e/run_all.sh` is a selected suite, not every tracked E2E script.

Do not claim verification that was not run. Report skipped checks and required
services or credentials explicitly.
