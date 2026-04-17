# runtara-environment

Control-plane server for Runtara — image registry, instance lifecycle, container execution, and durable-sleep wake scheduling.

## What it is

`runtara-environment` is the management-plane service for a Runtara deployment. It owns the image registry (upload, list, delete workflow binaries), drives the instance lifecycle (start, stop, resume, signal), spawns workflow containers through pluggable runner backends (OCI, native, or WASM), and runs the wake scheduler that resumes suspended instances when durable sleeps expire.

It persists images, instances, and the wake queue in PostgreSQL, sharing the pool with `runtara-core` so its migrations layer cleanly on top of the core schema. A set of background workers (cleanup, image GC, heartbeat monitoring, DB cleanup) run alongside the HTTP server.

The crate ships both a binary (`runtara-environment`, default port 8002, serving the Environment HTTP protocol) and a library. The library exposes a `runtime::EnvironmentRuntime` builder plus module-level building blocks (`config`, `runner`, `image_registry`, `wake_scheduler`, `migrations`) for embedding Environment inside another process.

## Using it standalone

**Variant B (server binary / internal):** This is a service component of a Runtara deployment, not a general-purpose library. Operators run it as the `runtara-environment` binary against a PostgreSQL database reachable by `runtara-core`; it boots via `Config::from_env()` and applies `migrations::run()` before starting its HTTP listener.

Clients don't call it directly — they go through `runtara-management-sdk`, which speaks the Environment protocol on behalf of CLIs and tooling. For an all-in-one deployment, the `runtara-server` crate embeds Environment in-process via `EnvironmentRuntime::builder()`, so most users never run this binary on its own. Deployment details (environment variables, data-directory layout, runner-specific requirements) live in the operator documentation rather than here.

## Inside Runtara

- **Consumers:** `runtara-server` (embeds `EnvironmentRuntime` in-process for the single-binary deployment) and `runtara-management-sdk` (client to the Environment HTTP protocol).
- **Key workspace deps:** `runtara-core` (shared `Persistence` trait, PostgreSQL pool, signal storage) and `runtara-dsl` (agent metadata types used by `list_agents` / `get_capability` handlers).
- **Integration point:** Environment orchestrates the workflow instance lifecycle on top of `runtara-core`'s persistence — it spawns containers via the `runner::Runner` trait and proxies cancel/pause/resume signals to core, which stores them for the running instance to consume at its next checkpoint.
- **Runner backends:** Pluggable via the `runner::Runner` trait. OCI (runc) is the production default; native-process and WASM runners are available for development and constrained environments.
- **Background workers:** `cleanup_worker`, `db_cleanup_worker`, `image_cleanup_worker`, and `heartbeat_monitor` run as tokio tasks inside the runtime, reclaiming disk, pruning stale rows, and failing instances whose heartbeat stops.
- **Runs in:** native host binary (requires OCI tooling on the host when the OCI runner is selected).

## License

AGPL-3.0-or-later.
