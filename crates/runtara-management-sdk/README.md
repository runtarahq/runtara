# runtara-management-sdk

[![Crates.io](https://img.shields.io/crates/v/runtara-management-sdk.svg)](https://crates.io/crates/runtara-management-sdk)
[![Docs.rs](https://docs.rs/runtara-management-sdk/badge.svg)](https://docs.rs/runtara-management-sdk)

Ergonomic async client for managing Runtara deployments over HTTP.

## What it is

A client SDK for driving a `runtara-environment` server: register/list/delete images,
start/stop/resume instances, send signals (pause, cancel), query checkpoints, events,
step summaries, and tenant metrics. All operations flow through `ManagementSdk`, a
small `reqwest`-based facade over Environment's HTTP/JSON API; signals targeting
workflow internals are proxied by Environment onward to `runtara-core`. Configuration
lives in `SdkConfig` (address + timeouts, constructible via `::localhost()` or
`::from_env()`), and every request returns typed structs from the `types` module
(e.g. `StartInstanceResult`, `InstanceStatus`, `ListInstancesResult`).

## Using it standalone

Add to `Cargo.toml`:

```toml
[dependencies]
runtara-management-sdk = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Start an instance against a local environment server:

```rust,no_run
use runtara_management_sdk::{ManagementSdk, StartInstanceOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = ManagementSdk::localhost()?;
    sdk.connect().await?;

    let opts = StartInstanceOptions::new("my-image-id", "tenant-1")
        .with_input(serde_json::json!({"key": "value"}));
    let started = sdk.start_instance(opts).await?;

    let status = sdk.get_instance_status(&started.instance_id).await?;
    println!("{:?}", status.status);
    Ok(())
}
```

Prerequisite: a reachable `runtara-environment` HTTP endpoint (default
`127.0.0.1:8002`, override via `RUNTARA_ENVIRONMENT_ADDR`).

## Inside Runtara

- Ships the `runtara-ctl` binary — the operator CLI used to drive a running
  Environment for image and instance management.
- Ships the `e2e-parallel-status-test` binary used by the end-to-end test harness.
- Consumed as a library by `runtara-server` for in-process management calls.
- Primary integration point: HTTP client for `runtara-environment`'s management
  API (`runtara-environment/src/http_server.rs`); signal operations are proxied
  by Environment to `runtara-core`.
- Runs on the native host (CLI and test binaries link against `reqwest` with
  `rustls-tls`); it is not a WASM target.

## License

AGPL-3.0-or-later.
