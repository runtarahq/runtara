// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-mediated HTTP transport — `runtara:host-io/http.request`.
//!
//! The wasip3-parallelism route (b) (docs/wasip3-parallelism.md §3.6): the
//! guest hands the whole buffered request to ONE host import whose host-side
//! binding is `func_wrap_concurrent`, so a pending request parks only the
//! CALLING task. Concurrent Split subtasks therefore overlap their agent
//! HTTP I/O — unlike the p2 `wasi:http` binding, whose pollable waits hold
//! the whole store (`func_wrap_async`).
//!
//! Only PROXIED calls (`call_agent` with `RUNTARA_HTTP_PROXY_URL` set — the
//! production path for every agent request) ride this import; direct
//! `call()`s keep the standard `wasi:http` transport, so plain SDK/local use
//! never requires the host binding.
//!
//! The import is async-TYPED (blocking is legal for its callers under ABI v2)
//! but sync-LOWERED here (`async: false`): the agent's task blocks until the
//! host future resolves.

use std::collections::HashMap;

use crate::{Body, HttpError, HttpResponse, RequestBuilder};

#[allow(warnings)]
mod bindings {
    wit_bindgen::generate!({
        inline: "
            package runtara:host-io@0.1.0;

            interface http {
                /// Buffered request/response envelopes (JSON bytes):
                ///   input:  { method, url, headers: [[k,v]...], body_b64 }
                ///   output: { status, headers: [[k,v]...], body_b64 }
                /// Err carries a transport-level message.
                request: async func(input: list<u8>) -> result<list<u8>, string>;
            }

            world host-io-client {
                import http;
            }
        ",
        world: "host-io-client",
        async: false,
    });
}

pub(crate) fn execute(request: RequestBuilder) -> Result<HttpResponse, HttpError> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let body_b64 = match &request.body {
        Some(Body::Json(value)) => {
            Some(BASE64.encode(serde_json::to_vec(value).map_err(|error| {
                HttpError::Transport(format!("serialize host-io body: {error}"))
            })?))
        }
        Some(Body::Bytes(bytes)) => Some(BASE64.encode(bytes)),
        None => None,
    };
    let input = serde_json::json!({
        "method": request.method,
        "url": request.url,
        "headers": request.headers,
        "body_b64": body_b64,
        "timeout_ms": request.timeout.map(|t| t.as_millis() as u64),
    });
    let input =
        serde_json::to_vec(&input).map_err(|error| HttpError::Transport(error.to_string()))?;

    let output = bindings::runtara::host_io::http::request(&input).map_err(HttpError::Transport)?;
    let envelope: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|error| HttpError::Transport(format!("parse host-io response: {error}")))?;

    let status = envelope["status"].as_u64().unwrap_or(0) as u16;
    let headers: HashMap<String, String> = envelope["headers"]
        .as_array()
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|pair| {
                    Some((
                        pair.get(0)?.as_str()?.to_string(),
                        pair.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let body = match envelope["body_b64"].as_str() {
        Some(raw) => BASE64
            .decode(raw)
            .map_err(|error| HttpError::Transport(format!("host-io body base64: {error}")))?,
        None => Vec::new(),
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}
