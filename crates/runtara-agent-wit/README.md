# runtara-agent-wit

[![Crates.io](https://img.shields.io/crates/v/runtara-agent-wit.svg)](https://crates.io/crates/runtara-agent-wit)
[![Docs.rs](https://docs.rs/runtara-agent-wit/badge.svg)](https://docs.rs/runtara-agent-wit)

The canonical `runtara:agent@0.1.0` WIT package. Every runtara agent component implements this contract; the host (`runtara-component-host`) consumes it.

## What's in here

- [`wit/runtara-agent.wit`](wit/runtara-agent.wit) — the package definition. One world (`agent`) exporting one interface (`capabilities`) with three functions: `get-module-info`, `list-capabilities`, `invoke`.
- [`wit/deps.toml`](wit/deps.toml) + [`wit/deps/`](wit/deps/) — pinned WASI 0.2.3 dependencies (http, cli, clocks, io, random, filesystem, sockets), managed by [`wit-deps`](https://github.com/bytecodealliance/wit-deps).
- [`src/lib.rs`](src/lib.rs) — exposes `RUNTARA_AGENT_WIT: &str` (the WIT source baked in via `include_str!`) so consumers can reference the contract without filesystem lookups.

## Bumping the contract

Edit `wit/runtara-agent.wit` and update `package runtara:agent@<X.Y.Z>;` per the semver policy:

| Change                              | Bump      |
| ----------------------------------- | --------- |
| New optional record field           | `0.1.x → 0.1.(x+1)` |
| New function on `capabilities`      | `0.1.x → 0.1.(x+1)` |
| Renamed / removed field, signature change | `0.x → 0.(x+1).0` |
| First externally-stable contract    | `0.y → 1.0.0` |

The host and every guest must depend on the same package version; wasmtime refuses to link a mismatched world at instantiation. CI verifies host/guest agreement on the version.

## Refreshing WASI deps

```bash
cd crates/runtara-agent-wit
wit-deps lock        # refresh wit/deps/ from upstream wasi-* repos
```

The lockfile (`wit/deps.lock`) and unpacked `wit/deps/` directory are committed so builds are hermetic — no network fetch at compile time.

## Why no `runtara:host` interface

Agents use typed Runtara host interfaces for outbound HTTP, connections, SQL, and runtime services. Outbound requests import `runtara:outbound-http/client@0.1.0`; the host injects credentials using an opaque connection ID and host-owned tenant identity. Raw `wasi:http` is denied. Logging uses `wasi:cli/stderr`. Approved built-in trusted capabilities receive credentials only in fresh restricted instances with network access denied.

## Guest usage

The guest's per-agent crate (e.g. `runtara-agent-crypto`) imports this WIT via `cargo component`'s metadata:

```toml
[package.metadata.component.target]
path = "../runtara-agent-wit/wit"
world = "agent"
```

## Host usage

`runtara-component-host` calls `wasmtime::component::bindgen!` against this WIT to generate the host-side types (`Agent`, `AgentPre`, `CapabilityInfo`, `ConnectionInfo`, `ErrorInfo`, `ModuleInfo`).
