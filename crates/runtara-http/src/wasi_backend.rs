// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WASI HTTP backend using `wasi:http/outgoing-handler`.
//!
//! This module is only compiled when targeting `wasm32-wasip2` (or any
//! `target_family = "wasm"` target). It implements the same public API as the
//! native ureq backend so that callers are unaware of the underlying transport.

use std::collections::HashMap;
use std::time::Duration;

use wasi::http::outgoing_handler;
use wasi::http::types::{
    Fields, FutureIncomingResponse, Method, OutgoingBody, OutgoingRequest, RequestOptions, Scheme,
};
use wasi::io::poll::poll;

use crate::{Body, HttpError, HttpResponse, RequestBuilder};

/// WASI HTTP client.
///
/// Unlike the native backend there is no persistent agent — each request is
/// independent. The struct exists only for API compatibility.
#[derive(Clone)]
pub struct WasiHttpClient {
    timeout: Option<Duration>,
}

impl WasiHttpClient {
    /// Create a new client with default settings.
    pub fn new() -> Self {
        Self { timeout: None }
    }

    /// Create a new client with a custom timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
        }
    }

    /// Start building a request.
    pub fn request(&self, method: &str, url: &str) -> RequestBuilder {
        let mut rb = RequestBuilder::new(method, url);
        if let Some(t) = self.timeout {
            rb.timeout = Some(t);
        }
        rb
    }
}

impl Default for WasiHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// URL parsing helpers
// ---------------------------------------------------------------------------

struct ParsedUrl {
    scheme: Scheme,
    authority: String,
    path_and_query: String,
}

/// Minimal URL parser that extracts scheme, authority, and path+query.
fn parse_url(url: &str) -> Result<ParsedUrl, HttpError> {
    // Determine scheme
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (Scheme::Https, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (Scheme::Http, rest)
    } else if let Some((s, rest)) = url.split_once("://") {
        (Scheme::Other(s.to_string()), rest)
    } else {
        return Err(HttpError::Transport(format!("URL missing scheme: {url}")));
    };

    // Split authority from path+query
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    if authority.is_empty() {
        return Err(HttpError::Transport(format!(
            "URL missing authority: {url}"
        )));
    }

    Ok(ParsedUrl {
        scheme,
        authority: authority.to_string(),
        path_and_query: path_and_query.to_string(),
    })
}

/// Simple percent-encoding for query parameter keys/values.
fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            _ => {
                for byte in c.to_string().as_bytes() {
                    result.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Method conversion
// ---------------------------------------------------------------------------

fn to_wasi_method(method: &str) -> Method {
    match method.to_uppercase().as_str() {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "CONNECT" => Method::Connect,
        "OPTIONS" => Method::Options,
        "TRACE" => Method::Trace,
        "PATCH" => Method::Patch,
        other => Method::Other(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Execute
// ---------------------------------------------------------------------------

/// Execute a request using the WASI HTTP outgoing-handler.
pub(crate) fn execute(builder: RequestBuilder) -> Result<HttpResponse, HttpError> {
    // -- Build the full URL with query params ---------------------------------
    let full_url = if builder.query_params.is_empty() {
        builder.url.clone()
    } else {
        let qs: String = builder
            .query_params
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        if builder.url.contains('?') {
            format!("{}&{qs}", builder.url)
        } else {
            format!("{}?{qs}", builder.url)
        }
    };

    let parsed = parse_url(&full_url)?;

    // -- Build headers --------------------------------------------------------
    let mut header_entries: Vec<(String, Vec<u8>)> = builder
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.as_bytes().to_vec()))
        .collect();

    // Determine the body bytes and maybe add Content-Type
    let body_bytes: Option<Vec<u8>> = match &builder.body {
        Some(Body::Json(value)) => {
            let has_ct = builder
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
            if !has_ct {
                header_entries.push(("content-type".to_string(), b"application/json".to_vec()));
            }
            Some(serde_json::to_vec(value)?)
        }
        Some(Body::Bytes(data)) => Some(data.clone()),
        None => None,
    };

    let fields = Fields::from_list(&header_entries)
        .map_err(|e| HttpError::Transport(format!("Invalid headers: {e:?}")))?;

    // -- Build OutgoingRequest ------------------------------------------------
    let request = OutgoingRequest::new(fields);

    request
        .set_method(&to_wasi_method(&builder.method))
        .map_err(|()| HttpError::Transport("Failed to set HTTP method".to_string()))?;

    request
        .set_scheme(Some(&parsed.scheme))
        .map_err(|()| HttpError::Transport("Failed to set scheme".to_string()))?;

    request
        .set_authority(Some(&parsed.authority))
        .map_err(|()| HttpError::Transport("Failed to set authority".to_string()))?;

    request
        .set_path_with_query(Some(&parsed.path_and_query))
        .map_err(|()| HttpError::Transport("Failed to set path".to_string()))?;

    // -- Write request body ---------------------------------------------------
    if let Some(bytes) = &body_bytes {
        let outgoing_body = request
            .body()
            .map_err(|()| HttpError::Transport("Failed to get outgoing body".to_string()))?;
        {
            let stream = outgoing_body.write().map_err(|()| {
                HttpError::Transport("Failed to get body output stream".to_string())
            })?;
            // WASI limits blocking_write_and_flush to 4096 bytes per call.
            // Write in chunks to handle larger payloads.
            let mut offset = 0;
            while offset < bytes.len() {
                let end = (offset + 4096).min(bytes.len());
                stream
                    .blocking_write_and_flush(&bytes[offset..end])
                    .map_err(|e| {
                        HttpError::Transport(format!("Failed to write body chunk: {e}"))
                    })?;
                offset = end;
            }
            // stream must be dropped before finishing the body
        }
        OutgoingBody::finish(outgoing_body, None)
            .map_err(|e| HttpError::Transport(format!("Failed to finish outgoing body: {e}")))?;
    }

    // -- Build request options (timeouts) ------------------------------------
    let options = builder.timeout.map(|t| {
        let opts = RequestOptions::new();
        let nanos = t.as_nanos() as u64;
        // Best-effort: ignore errors if the runtime doesn't support these
        let _ = opts.set_connect_timeout(Some(nanos));
        let _ = opts.set_first_byte_timeout(Some(nanos));
        let _ = opts.set_between_bytes_timeout(Some(nanos));
        opts
    });

    // -- Send the request -----------------------------------------------------
    let future_resp: FutureIncomingResponse = outgoing_handler::handle(request, options)
        .map_err(|e| HttpError::Transport(format!("outgoing-handler error: {e}")))?;

    // -- Block on the response ------------------------------------------------
    let incoming_response = block_on_future_response(&future_resp)?;

    // -- Read status and headers ----------------------------------------------
    let status = incoming_response.status();

    let headers = {
        let fields = incoming_response.headers();
        let mut map = HashMap::new();
        for (name, value) in fields.entries() {
            let val = String::from_utf8_lossy(&value).to_string();
            map.insert(name.to_lowercase(), val);
        }
        map
    };

    // -- Read response body ---------------------------------------------------
    let body = read_incoming_body(&incoming_response)?;

    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

/// Block until the `FutureIncomingResponse` resolves and return the
/// `IncomingResponse`, or propagate any error.
fn block_on_future_response(
    future_resp: &FutureIncomingResponse,
) -> Result<wasi::http::types::IncomingResponse, HttpError> {
    // Poll until the response is ready
    loop {
        match future_resp.get() {
            Some(result) => {
                // Outer Result: from the future itself
                let inner = result.map_err(|()| {
                    HttpError::Transport("Future response already consumed".to_string())
                })?;
                // Inner Result: HTTP-level error code
                return inner.map_err(|e| HttpError::Transport(format!("HTTP error: {e}")));
            }
            None => {
                // Not ready yet — block via poll
                let pollable = future_resp.subscribe();
                poll(&[&pollable]);
            }
        }
    }
}

/// Read the full body from an `IncomingResponse`.
fn read_incoming_body(
    response: &wasi::http::types::IncomingResponse,
) -> Result<Vec<u8>, HttpError> {
    let incoming_body = response
        .consume()
        .map_err(|()| HttpError::Transport("Failed to consume response body".to_string()))?;

    let stream = incoming_body
        .stream()
        .map_err(|()| HttpError::Transport("Failed to get body input stream".to_string()))?;

    let mut body = Vec::new();
    loop {
        // Try to read a chunk (up to 64 KiB at a time)
        match stream.blocking_read(65536) {
            Ok(chunk) => {
                if chunk.is_empty() {
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => {
                break;
            }
            Err(e) => {
                return Err(HttpError::Transport(format!(
                    "Failed to read response body: {e}"
                )));
            }
        }
    }

    // Drop the stream before finishing the body
    drop(stream);

    // Finish the incoming body (consumes it, returns FutureTrailers which we ignore)
    let _trailers = wasi::http::types::IncomingBody::finish(incoming_body);

    Ok(body)
}
