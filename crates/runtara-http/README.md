# runtara-http

[![Crates.io](https://img.shields.io/crates/v/runtara-http.svg)](https://crates.io/crates/runtara-http)
[![Docs.rs](https://docs.rs/runtara-http/badge.svg)](https://docs.rs/runtara-http)

Blocking HTTP client that runs identically on native, WASI (wasm32-wasip2), and other wasm targets.

## What it is

A thin HTTP client abstraction with a single `HttpClient` / `RequestBuilder` / `HttpResponse` surface that compiles on every target Runtara supports. The backend is selected at compile time by `cfg(target_family = "wasm")`: native builds use [`ureq`] (with TLS), wasm builds use [`wasi:http/outgoing-handler`] via the `wasi` crate pinned to the WASI 0.2.4 ABI that Rust's std ships. There are no cargo features — picking the right target picks the right backend automatically. On top of direct `call()`, `call_agent()` will transparently forward a request through a proxy endpoint when `RUNTARA_HTTP_PROXY_URL` is set, serialising the request as JSON and letting the proxy inject credentials based on `X-Runtara-Connection-Id`. Non-2xx responses are returned as `Ok`; only transport failures produce `Err`.

## Using it standalone

```toml
[dependencies]
runtara-http = "4.0"
```

```rust
use runtara_http::HttpClient;
use std::time::Duration;

let client = HttpClient::with_timeout(Duration::from_secs(10));
let resp = client
    .request("GET", "https://api.example.com/items")
    .header("Authorization", "Bearer token")
    .query("page", "1")
    .call()?;

let items: serde_json::Value = resp.into_json()?;
```

Same code compiles for `cargo build` and `cargo build --target wasm32-wasip2`.

## Inside Runtara

- Declared as a `[workspace.dependencies]` entry (`runtara-http = { path = "crates/runtara-http", version = "4.0" }`) and consumed directly by `runtara-sdk`, `runtara-agents`, `runtara-ai`, and `runtara-workflow-stdlib`.
- `runtara-sdk` wraps it in `backend::http` as the transport for all SDK-side calls into the Runtara control plane; agent crates use `call_agent()` so credentialed outbound HTTP flows through the capability proxy.
- Depends only on `ureq` (native, TLS+JSON features) and a pinned `wasi = "=0.14.2"` (WASI 0.2.4 / wasip2 1.0.1, matching the Rust std ABI) plus `serde`, `serde_json`, `base64`, and `thiserror`.
- Runs in every process shape Runtara targets: the server/SDK on native x86_64 and aarch64 Linux, agent WASM components executed inside the Wasmtime-backed runtime on `wasm32-wasip2`, and browser-side wasm consumers that reuse the same API.
- Proxy protocol (`call_agent` + `RUNTARA_HTTP_PROXY_URL`) is the integration point with the connection-manager / agent HTTP proxy: `X-Runtara-Connection-Id` is stripped from the forwarded headers, `X-Org-Id` is forwarded (or taken from `RUNTARA_TENANT_ID`), and binary bodies round-trip as base64.

## License

AGPL-3.0-or-later.

[`ureq`]: https://docs.rs/ureq
[`wasi:http/outgoing-handler`]: https://github.com/WebAssembly/wasi-http
