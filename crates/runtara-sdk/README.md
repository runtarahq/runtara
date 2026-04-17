[![crates.io](https://img.shields.io/crates/v/runtara-sdk.svg)](https://crates.io/crates/runtara-sdk)
[![docs.rs](https://docs.rs/runtara-sdk/badge.svg)](https://docs.rs/runtara-sdk)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)

# runtara-sdk

High-level client library for building durable workflow instances that talk to `runtara-core`.

## What it is

`runtara-sdk` is the ergonomic surface a workflow/instance uses to register itself, checkpoint state, send lifecycle events (heartbeat, completed, failed, suspended), and poll for cancel/pause/resume signals. The central type is `RuntaraSdk`, built from env via `RuntaraSdk::from_env()` or programmatically via `HttpSdkConfig`; `checkpoint()` returns a `CheckpointResult` that distinguishes fresh execution from resume and carries any pending `Signal`. The `#[durable]` proc-macro (re-exported from `runtara-sdk-macros`) wires instance code into a global SDK registry so long-running operations can be cancelled cooperatively.

## Using it standalone

```toml
[dependencies]
runtara-sdk = "1.8"
```

```rust
use runtara_sdk::RuntaraSdk;

fn main() -> runtara_sdk::Result<()> {
    let mut sdk = RuntaraSdk::from_env()?;
    sdk.connect()?;
    sdk.register(None)?;

    let state = serde_json::to_vec(&"step-1-done")?;
    let result = sdk.checkpoint("step-1", &state)?;
    if result.should_cancel() {
        return Err(runtara_sdk::SdkError::Cancelled);
    }

    sdk.completed(b"ok")?;
    Ok(())
}
```

Requires `RUNTARA_INSTANCE_ID` and `RUNTARA_TENANT_ID` in the environment, plus a reachable `runtara-core` HTTP endpoint (defaults to `http://127.0.0.1:8003`). Enable `embedded` for in-process persistence or `wasi` / `wasm-js` for WASM targets.

## Inside Runtara

- Consumed by `runtara-workflow-stdlib`, which re-exports the SDK with `http` features for workflow authors.
- Depends on `runtara-sdk-macros` (the `#[durable]` proc-macro) and optionally on `runtara-core` (embedded mode) and `runtara-http` (HTTP mode).
- Main integration point: the `HttpBackend` calls `runtara-core`'s instance HTTP API; the global registry in `registry.rs` mediates signal delivery so `#[durable]` functions can observe cancellation mid-flight.
- Runs in: WASM guest (primary) / native host. The `wasi` and `wasm-js` features target `wasm32-wasip2` and `wasm32-unknown-unknown`; native hosts use the default `http` + `embedded` build.

## License

AGPL-3.0-or-later.
