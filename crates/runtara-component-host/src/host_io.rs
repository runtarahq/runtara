// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host side of `runtara:host-io/http` — the concurrent HTTP hop for agent
//! requests (docs/wasip3-parallelism.md §3.6 route (b)).
//!
//! Agents' PROXIED requests (`runtara-http::call_agent` under WASI) arrive
//! here as one buffered JSON envelope; the host performs the actual dial with
//! a native hyper client and returns the buffered response. The binding is
//! `func_wrap_concurrent`, so a pending request parks ONLY the calling guest
//! task — sibling subtasks (a parallel Split window) keep running. The p2
//! `wasi:http` path can't do this: its pollable waits are `func_wrap_async`,
//! which holds the whole store.
//!
//! Envelope contract (mirrored in `runtara-http/src/host_io.rs`):
//!   request:  `{ method, url, headers: [[k,v]…], body_b64, timeout_ms }`
//!   response: `{ status, headers: [[k,v]…], body_b64 }`
//!   Err(string) for transport-level failures (connect/timeout/protocol).

use std::time::Duration;

use anyhow::Result;
use wasmtime::component::Linker;

/// Ceiling for a single host-io request when the guest names no timeout —
/// matches the outer watchdog's order of magnitude so a hung upstream can't
/// pin a task forever.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn add_host_io_to_linker<T: Send + 'static>(linker: &mut Linker<T>) -> Result<()> {
    let mut instance = linker.instance("runtara:host-io/http@0.1.0")?;
    instance.func_wrap_concurrent("request", |_accessor, (input,): (Vec<u8>,)| {
        Box::pin(async move {
            let response: Result<Vec<u8>, String> = execute(input).await;
            Ok((response,))
        })
    })?;
    Ok(())
}

async fn execute(input: Vec<u8>) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use http_body_util::BodyExt;

    let envelope: serde_json::Value =
        serde_json::from_slice(&input).map_err(|error| format!("host-io envelope: {error}"))?;
    let method = envelope["method"].as_str().unwrap_or("GET").to_string();
    let url = envelope["url"]
        .as_str()
        .ok_or_else(|| "host-io envelope missing url".to_string())?
        .to_string();
    let body = match envelope["body_b64"].as_str() {
        Some(raw) => BASE64
            .decode(raw)
            .map_err(|error| format!("host-io body base64: {error}"))?,
        None => Vec::new(),
    };
    let timeout = envelope["timeout_ms"]
        .as_u64()
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT);

    let mut request = hyper::Request::builder().method(method.as_str()).uri(&url);
    if let Some(pairs) = envelope["headers"].as_array() {
        for pair in pairs {
            if let (Some(name), Some(value)) = (
                pair.get(0).and_then(|v| v.as_str()),
                pair.get(1).and_then(|v| v.as_str()),
            ) {
                request = request.header(name, value);
            }
        }
    }
    let request = request
        .body(http_body_util::Full::new(bytes::Bytes::from(body)))
        .map_err(|error| format!("host-io request build: {error}"))?;

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http::<http_body_util::Full<bytes::Bytes>>();

    let response = tokio::time::timeout(timeout, client.request(request))
        .await
        .map_err(|_| format!("host-io request timed out after {timeout:?}"))?
        .map_err(|error| format!("host-io request failed: {error}"))?;

    let status = response.status().as_u16();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).to_string(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|error| format!("host-io response body: {error}"))?
        .to_bytes();

    serde_json::to_vec(&serde_json::json!({
        "status": status,
        "headers": headers,
        "body_b64": BASE64.encode(&body),
    }))
    .map_err(|error| format!("host-io response envelope: {error}"))
}
