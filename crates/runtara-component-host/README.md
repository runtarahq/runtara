# runtara-component-host

[![Crates.io](https://img.shields.io/crates/v/runtara-component-host.svg)](https://crates.io/crates/runtara-component-host)
[![Docs.rs](https://docs.rs/runtara-component-host/badge.svg)](https://docs.rs/runtara-component-host)

Embedded wasmtime host for runtara agent components. Loads `runtara_agent_*.wasm` files from disk, instantiates them via the WIT `runtara:agent@0.1.0` contract, and dispatches `test_capability` calls in sub-millisecond time.

## What it is

A small library (`Engine` + `Linker` + `Store` + dispatcher service) that replaces the legacy "compile a Rust binary, register it as an OCI image, spawn `wasmtime run` per call" model. Per-call cost drops from ~600 ms (image-registry round-trip + container spawn) to <1 ms (`pre.instantiate_async` + `invoke`).

The crate exposes one user-facing service: `ComponentDispatcherService`. Internally it shares a process-wide `wasmtime::Engine` (component-model + async + epoch-interruption on) and a single `Linker` configured with `wasmtime_wasi::p2::add_to_linker_async` + `wasmtime_wasi_http::p2::add_only_http_to_linker_async` to satisfy WASI imports each guest agent declares.

## Quick start

```toml
[dependencies]
runtara-component-host = { path = "../runtara-component-host" }
```

```rust
use runtara_component_host::{
    ComponentDispatcherService, DispatcherEnv, TestCapabilityRequest,
};

let env = DispatcherEnv {
    proxy_url: "http://127.0.0.1:7002/api/internal/proxy".into(),
    agent_service_url: "http://127.0.0.1:7002/api/internal/agents".into(),
    object_model_url: "http://127.0.0.1:7002/api/internal/object-model".into(),
    core_http_url: "http://127.0.0.1:7002".into(),
};

let dispatcher = ComponentDispatcherService::from_dir(
    std::path::Path::new("./target/wasm32-wasip1/release"),
    env,
).await?;

let result = dispatcher.test_capability(TestCapabilityRequest {
    tenant_id:     "tenant-1".into(),
    agent_id:      "crypto".into(),
    capability_id: "hash".into(),
    input:         serde_json::json!({ "data": "hello" }),
    connection:    None,
}).await?;

println!("{}", result.output.unwrap());  // {"hash":"2cf24...","algorithm":"sha256","format":"hex"}
```

## How discovery works

`from_dir` scans for `runtara_agent_*.wasm` files; the stem after `runtara_agent_` becomes the agent id. One `Component::from_file` parse + Cranelift compile happens per agent at startup; per-call, `AgentPre::instantiate_async` constructs a fresh `Store<HostState>` and runs `invoke` — no extra parsing or compilation.

## Security posture

`HostState::send_request` (in `host_state.rs`) intercepts every outbound HTTP request from a guest and:

- Forces `X-Org-Id: <tenant_id>` from the host — overrides any value the guest set, closing the "tampered SDK could spoof tenancy" hole.
- Strips `Authorization` and `Cookie` headers from requests that don't target the configured proxy host — credentials must flow through the proxy that injects them, never directly from the agent.

## Where it slots in

`runtara-server` builds a `ComponentDispatcherService` at boot when `RUNTARA_AGENT_COMPONENTS_DIR` is set, plugs it into `AgentTestingService`, and routes `POST /api/runtime/agents/{name}/capabilities/{cap}/test?engine=components` through this crate. See [`docs/wasm-components-migration-plan.md` § 6](../../docs/wasm-components-migration-plan.md).
