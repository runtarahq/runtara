# runtara-environment

Control-plane library for Runtara — image registry, instance lifecycle, workflow execution, and durable-sleep wake scheduling.

## What it is

`runtara-environment` is the management-plane service for a Runtara deployment. It owns the image registry (upload, list, delete workflow binaries), drives the instance lifecycle (start, stop, resume, signal), executes workflows on an in-process wasmtime engine, and runs the wake scheduler that resumes suspended instances when durable sleeps expire.

It persists images, instances, and the wake queue in PostgreSQL, sharing the pool with `runtara-core` so its migrations layer cleanly on top of the core schema. A set of background workers (cleanup, image GC, heartbeat monitoring, DB cleanup) run as tokio tasks inside the runtime.

It is a library, not a service: no HTTP, no sockets, no binary. The protocol is a set of async functions in `handlers` over a shared `EnvironmentHandlerState`, and `runtime::EnvironmentRuntime` owns the workers.

## Using it

Build a runtime with `EnvironmentRuntime::builder()`, supplying the pool, a `runner::Runner`, core persistence and a data directory, then call `migrations::run()` before starting it. Drive it by calling the `handlers` functions.

`runtara-server` is the only consumer: it calls these handlers directly through its `environment_client` module, in the same process. There is no management wire protocol any more.

## Inside Runtara

- **Consumers:** `runtara-server`, which embeds `EnvironmentRuntime` in-process and calls the `handlers` functions directly.
- **Key workspace deps:** `runtara-core` (shared `Persistence` trait, PostgreSQL pool, signal storage) and `runtara-dsl` (agent metadata types used by `list_agents` / `get_capability` handlers).
- **Integration point:** Environment orchestrates the workflow instance lifecycle on top of `runtara-core`'s persistence — it launches workflow runs via the `runner::Runner` trait and proxies cancel/pause/resume signals to core, which stores them for the running instance to consume at its next checkpoint.
- **Runner backend:** `EmbeddedWasmRunner` — an in-process wasmtime engine — is the only backend. `MockRunner` exists for tests. The `runner::Runner` trait keeps that seam.
- **Background workers:** `cleanup_worker`, `db_cleanup_worker`, `image_cleanup_worker`, and `heartbeat_monitor` run as tokio tasks inside the runtime, reclaiming disk, pruning stale rows, and failing instances whose heartbeat stops.
- **No web dependencies:** the crate pulls in no HTTP framework — that belongs to whichever host serves it.
- **Runs in:** the host process that embeds it; workflow guests run in-process as WASM components, so there is no container runtime to install.

## License

AGPL-3.0-or-later.
