// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host side of `runtara:host-io/http` — the concurrent HTTP hop for agent
//! requests.
//!
//! Requests from `runtara-http` under WASI arrive here as one buffered JSON
//! envelope; the host performs the actual dial with a native hyper client and
//! returns the buffered response. The binding is `func_wrap_concurrent`, so a
//! pending request parks ONLY the calling guest task — sibling subtasks (a
//! parallel Split window) keep running. The p2 `wasi:http` path can't do this:
//! its pollable waits are `func_wrap_async`, which holds the whole store.
//!
//! This is deliberately the sole supported HTTP transport for Runtara
//! workflows and agents. It applies one absolute deadline to connecting,
//! receiving headers, and consuming the response body, and it enforces a
//! response-body ceiling. Raw `wasi:http` is rejected by the host hooks rather
//! than providing an unbounded alternate path.
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
pub(crate) const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(120);

/// A guest is allowed to ask for a shorter timeout, but never a longer one.
/// The active execution deadline can reduce it further (see
/// [`HostIoContext::http_deadline`]).
pub(crate) const MAX_HTTP_TIMEOUT: Duration = Duration::from_secs(120);

/// Buffered responses cross both the host/guest boundary and the workflow
/// data path. Large payloads belong in object storage rather than a runner's
/// memory. The streaming counter below remains authoritative even when a peer
/// lies about (or omits) `Content-Length`.
pub(crate) const MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Store data that exposes the enclosing active-execution deadline to
/// host-io. `None` is used by short-lived test/metadata stores; the per-call
/// HTTP policy still supplies its bounded default in that case.
pub(crate) trait HostIoContext {
    fn http_deadline(&self) -> Option<tokio::time::Instant>;
}

pub(crate) fn add_host_io_to_linker<T: HostIoContext + Send + 'static>(
    linker: &mut Linker<T>,
) -> Result<()> {
    let mut instance = linker.instance("runtara:host-io/http@0.1.0")?;
    instance.func_wrap_concurrent("request", |accessor, (input,): (Vec<u8>,)| {
        let active_deadline = accessor.with(|mut access| access.get().http_deadline());
        Box::pin(async move {
            let response: Result<Vec<u8>, String> = execute(input, active_deadline).await;
            Ok((response,))
        })
    })?;
    // Concurrent timer for in-window retry backoff: a sleep is just
    // another waitable in the window's set, so item backoffs overlap
    // instead of serializing through assembly.
    let mut timers = linker.instance("runtara:host-io/timers@0.1.0")?;
    timers.func_wrap_concurrent("sleep", |_accessor, (ms,): (u64,)| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        })
    })?;
    Ok(())
}

async fn execute(
    input: Vec<u8>,
    active_deadline: Option<tokio::time::Instant>,
) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
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
    let timeout = caller_timeout(envelope["timeout_ms"].as_u64())?;
    let deadline = active_deadline
        .map(|deadline| deadline.min(tokio::time::Instant::now() + timeout))
        .unwrap_or_else(|| tokio::time::Instant::now() + timeout);
    ensure_before_deadline(deadline)?;

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

    // `Client::request` covers DNS/connection setup and response headers. Do
    // not create a fresh duration for body collection below: both phases share
    // this same absolute deadline.
    let response = tokio::time::timeout_at(deadline, client.request(request))
        .await
        .map_err(|_| timeout_error())?
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
    if response_content_length_exceeds(response.headers(), MAX_RESPONSE_BODY_BYTES) {
        return Err(response_too_large(MAX_RESPONSE_BODY_BYTES));
    }
    let body = collect_response_body(response.into_body(), deadline).await?;

    serde_json::to_vec(&serde_json::json!({
        "status": status,
        "headers": headers,
        "body_b64": BASE64.encode(&body),
    }))
    .map_err(|error| format!("host-io response envelope: {error}"))
}

/// Resolve a guest-provided timeout against the host policy. A missing timeout
/// uses the policy default; a requested timeout is capped rather than allowed
/// to extend an active run. Zero is invalid rather than an accidental
/// unbounded timeout or a timing-dependent immediate success.
fn caller_timeout(requested_ms: Option<u64>) -> Result<Duration, String> {
    let Some(requested_ms) = requested_ms else {
        return Ok(DEFAULT_HTTP_TIMEOUT);
    };
    if requested_ms == 0 {
        return Err("host-io timeout_ms must be greater than zero".to_string());
    }
    Ok(Duration::from_millis(requested_ms).min(MAX_HTTP_TIMEOUT))
}

fn ensure_before_deadline(deadline: tokio::time::Instant) -> Result<(), String> {
    if deadline <= tokio::time::Instant::now() {
        return Err(timeout_error());
    }
    Ok(())
}

fn timeout_error() -> String {
    "host-io timeout: absolute request deadline elapsed".to_string()
}

fn response_too_large(limit: usize) -> String {
    format!("host-io response_too_large: limit is {limit} bytes")
}

fn response_content_length_exceeds(headers: &http::HeaderMap, limit: usize) -> bool {
    headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > limit as u64)
}

async fn collect_response_body(
    mut body: hyper::body::Incoming,
    deadline: tokio::time::Instant,
) -> Result<Vec<u8>, String> {
    use http_body_util::BodyExt;

    let mut bytes = Vec::new();
    loop {
        let frame = tokio::time::timeout_at(deadline, body.frame())
            .await
            .map_err(|_| timeout_error())?;
        let Some(frame) = frame else {
            return Ok(bytes);
        };
        let frame = frame.map_err(|error| format!("host-io response body: {error}"))?;
        let Ok(chunk) = frame.into_data() else {
            // Trailers are not part of the buffered response envelope.
            continue;
        };
        let remaining = MAX_RESPONSE_BODY_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            // Dropping the body cancels the HTTP stream rather than draining an
            // attacker-controlled response after the cap has been crossed.
            return Err(response_too_large(MAX_RESPONSE_BODY_BYTES));
        }
        bytes.extend_from_slice(&chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn envelope(url: &str, timeout_ms: u64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "method": "GET",
            "url": url,
            "timeout_ms": timeout_ms,
        }))
        .expect("serialize envelope")
    }

    async fn one_request_server(
        write_response: impl FnOnce(
            tokio::net::TcpStream,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let address = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.expect("read request");
            write_response(stream).await;
        });
        format!("http://{address}/")
    }

    #[test]
    fn caller_timeout_rejects_zero_and_caps_large_values() {
        assert!(caller_timeout(Some(0)).is_err());
        assert_eq!(caller_timeout(None).unwrap(), DEFAULT_HTTP_TIMEOUT);
        assert_eq!(
            caller_timeout(Some(u64::MAX)).unwrap(),
            MAX_HTTP_TIMEOUT,
            "a caller cannot extend the host HTTP policy"
        );
    }

    #[tokio::test]
    async fn local_server_body_stall_uses_the_original_deadline() {
        let url = one_request_server(|mut stream| {
            Box::pin(async move {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                    .await
                    .expect("write headers");
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = stream.write_all(b"2\r\nok\r\n0\r\n\r\n").await;
            })
        })
        .await;

        let error = execute(envelope(&url, 20), None)
            .await
            .expect_err("a stalled body must time out");
        assert_eq!(error, timeout_error());
    }

    #[tokio::test]
    async fn local_server_declared_oversize_body_is_rejected_without_buffering() {
        let url = one_request_server(|mut stream| {
            Box::pin(async move {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    MAX_RESPONSE_BODY_BYTES + 1
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write headers");
            })
        })
        .await;

        // This test exercises the header-only size rejection, not deadline
        // enforcement. Leave enough local scheduling margin for the server
        // task to accept and reply before the request reaches that check.
        let error = execute(envelope(&url, 5_000), None)
            .await
            .expect_err("a declared oversize body must be rejected");
        assert_eq!(error, response_too_large(MAX_RESPONSE_BODY_BYTES));
    }
}
