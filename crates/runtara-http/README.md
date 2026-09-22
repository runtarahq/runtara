# runtara-http

[![Crates.io](https://img.shields.io/crates/v/runtara-http.svg)](https://crates.io/crates/runtara-http)
[![Docs.rs](https://docs.rs/runtara-http/badge.svg)](https://docs.rs/runtara-http)

Blocking HTTP client that runs identically on native and WASI (wasm32-wasip2).

## What it is

One `HttpClient` / `RequestBuilder` / `HttpResponse` API supports native SDK calls
through `ureq` and WASM calls through `runtara:outbound-http/client@0.1.0`.
Enable exactly one backend feature: `native` or `wasi`.

All WASM entry points (`call`, `call_async`, `call_agent`, `call_agent_async`) use
the injected native outbound service. Async calls suspend only the calling guest
task and propagate cancellation. HTTP statuses are returned as responses.

## Using it standalone

```toml
[dependencies]
runtara-http = { version = "8.7", features = ["native"] }
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

The same code compiles for `cargo build --target wasm32-wasip2` with `features = ["wasi"]`.

## Inside Runtara

```rust,ignore
let response = HttpClient::new()
    .request("POST", "/items")
    .connection_id("opaque-connection-id")
    .body_json(&serde_json::json!({"name": "example"}))
    .call_agent_async()
    .await?;
```

The host resolves credentials and applies existing OAuth, signing, destination,
mTLS, and rate-limit behavior. Connection IDs, endpoint names/references, AI
providers, and AWS services are explicit builder fields. Tenant and instance
identity come from host context. A request without a connection ID is a public
request, including requests to provider-issued signed URLs.

The typed boundary carries raw bytes, with no JSON/base64 transport envelope or
internal HTTP listener. JSON bodies are serialized once before signing. The
host default deadline is 30 seconds, capped at 120 seconds and the execution's
remaining time. It covers credential lookup through response consumption.
Request and response budgets are 64 MiB and 8 MiB including metadata; download
helpers retain their 5 MiB limit. These budgets count raw bytes, so removing the
old encoded envelopes increases effective payload capacity.

The native SDK retains direct ordinary HTTP. A native connection-aware request
fails explicitly because that SDK has no injected credential service. Missing
WASM services also fail explicitly; there is no direct-network fallback.

Rebuild agents and composed workflows when upgrading from the old HTTP import.
Deploy and roll back matching host/component bundles together. Drain older
running or suspended artifacts on their matching release, or restart them
explicitly. Timer imports remain unchanged.

## License

AGPL-3.0-or-later.

[`ureq`]: https://docs.rs/ureq
