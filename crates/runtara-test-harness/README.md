# runtara-test-harness

Binary that executes a single agent capability in isolation, used by Runtara to exercise workflow stdlib capabilities inside OCI containers.

## What it is

A standalone binary (no library) that reads a JSON test request from `/data/input.json`, resolves an agent/capability pair via the `runtara-workflow-stdlib` registry, executes it, and writes an `InstanceOutput` to `/data/output.json`. It is the same execution shape as a real workflow step — the harness links against the full stdlib so any agent capability registered there is callable by name. Connection credentials, if present, are injected into the capability input as a `_connection` field before dispatch. The binary is not published (`publish = false`) and exposes no public Rust API — the contract is the on-disk JSON envelope.

## Using it standalone

Build with `cargo build --release -p runtara-test-harness`, mount an input file at `/data/input.json` and a writable `/data` directory, then run the binary. Input shape:

```json
{
  "agent_id": "http",
  "capability_id": "http-request",
  "input": { "url": "/api/users", "method": "GET" },
  "connection": {
    "integration_id": "bearer",
    "parameters": { "base_url": "https://api.example.com", "token": "secret" }
  }
}
```

Exit code is `0` on success, `1` on parse/exec failure; the full result (completed or failed) is written to `/data/output.json` in the standard `InstanceOutput` envelope.

## Inside Runtara

- Invoked operationally by `runtara-environment` (see `handle_test_capability` in `crates/runtara-environment/src/handlers.rs`) as an OCI image under the `__system__` tenant; not a library dependency of any crate.
- The compiled binary is picked up from `$DATA_DIR/test-harness/binary` or `/usr/share/runtara/test-harness` and registered once in the `ImageRegistry` on first use.
- Depends on `runtara-workflow-stdlib` (for the capability registry and all bundled agents) and `runtara-dsl` (agent metadata types).
- Runs inside the same OCI container runtime as production workflow instances, reusing `instance_output::{write_completed, write_failed}` so the runner treats results identically to real workflow launches.
- Primary use case: the environment service's `TestCapabilityRequest` endpoint, which lets callers validate a capability + connection combination without constructing a full workflow.

## License

AGPL-3.0-or-later.
