# runtara-component-host

[![Crates.io](https://img.shields.io/crates/v/runtara-component-host.svg)](https://crates.io/crates/runtara-component-host)
[![Docs.rs](https://docs.rs/runtara-component-host/badge.svg)](https://docs.rs/runtara-component-host)

Embedded Wasmtime host for Runtara agents and compiled workflows. The dispatcher
loads agent components and invokes capabilities; `WorkflowExecutor` executes
composed workflows and scoped child components. Both reuse compiled components
and create invocation-specific Stores.

Runtime, connection resolution, database and outbound HTTP behavior are injected
host services. The component host does not own credential storage or a network
proxy. Its concurrent imports let sibling guest tasks continue while I/O waits.

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
    core_http_url: "http://127.0.0.1:7002".into(),
};

let dispatcher = ComponentDispatcherService::from_dir(
    std::path::Path::new("./target/wasm32-wasip2/release"),
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

Raw WASI HTTP is denied. Guest HTTP uses
`runtara:outbound-http/client@0.1.0`, with explicit connection IDs or public URLs,
raw body bytes, bounded responses and the invocation's active deadline.
`OutboundHttpHost` receives tenant and instance identity from host-owned context;
guest headers and environment cannot override it. Missing services fail explicitly.

The server supplies credential resolution, OAuth refresh, request signing,
connection ownership, destination checks, mTLS and rate limiting. Ordinary agents
receive no stored credentials. Approved trusted capabilities run in fresh,
restricted instances that deny outbound requests before credential lookup or I/O.

Inject an outbound service with `dispatcher.set_outbound_http(service)` or
`executor.set_outbound_http(service)` before invoking network capabilities. Pure
capabilities such as the example above need no outbound service.

The old `runtara:host-io/http@0.1.0` import is removed; rebuild host, agent and
workflow artifacts together. The separate `runtara:host-io/timers@0.1.0` contract
is unchanged.

## Where it slots in

`runtara-server` builds a `ComponentDispatcherService` at boot when `RUNTARA_AGENT_COMPONENTS_DIR` is set, plugs it into `AgentTestingService`, and routes `POST /api/runtime/agents/{name}/capabilities/{cap}/test` through this crate.
