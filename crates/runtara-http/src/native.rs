// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native HTTP backend using ureq.

use std::collections::HashMap;
use std::io::Read as _;
use std::time::Duration;

use crate::{Body, HttpError, HttpResponse, RequestBuilder};

/// Native HTTP client backed by `ureq::Agent`.
#[derive(Clone)]
pub struct NativeHttpClient {
    agent: ureq::Agent,
}

impl NativeHttpClient {
    /// Create a new client with default settings.
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::new(),
        }
    }

    /// Create a new client with a custom timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new().timeout(timeout).build(),
        }
    }

    /// Start building a request.
    pub fn request(&self, method: &str, url: &str) -> RequestBuilder {
        let mut rb = RequestBuilder::new(method, url);
        rb.agent = Some(self.agent.clone());
        rb
    }
}

impl Default for NativeHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a request using the native ureq backend.
pub(crate) fn execute(builder: RequestBuilder) -> Result<HttpResponse, HttpError> {
    if builder.connection_id.is_some() {
        return Err(HttpError::Transport(
            "Connection-aware HTTP requires the outbound host service".into(),
        ));
    }
    // Build the agent: use the stored one or create a fresh one
    let agent = builder.agent.unwrap_or_else(ureq::Agent::new);

    // Build the URL with query parameters
    let url = crate::build_url_with_query(&builder.url, &builder.query_params);

    let mut request = agent.request(&builder.method, &url);

    // Apply per-request timeout if set
    if let Some(timeout) = builder.timeout {
        request = request.timeout(timeout);
    }

    // Apply headers
    for (key, value) in &builder.headers {
        request = request.set(key, value);
    }

    // Send with body
    let result = match builder.body {
        Some(Body::Json(ref value)) => {
            // Set content-type if not already set
            let has_ct = builder
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
            if !has_ct {
                request = request.set("Content-Type", "application/json");
            }
            request.send_json(value)
        }
        Some(Body::Bytes(ref data)) => request.send_bytes(data),
        None => request.call(),
    };

    match result {
        Ok(resp) => response_from_ureq(resp),
        Err(ureq::Error::Status(code, resp)) => response_from_ureq_with_status(code, resp),
        Err(ureq::Error::Transport(e)) => Err(HttpError::Transport(e.to_string())),
    }
}

/// Convert a successful ureq response into our HttpResponse.
fn response_from_ureq(resp: ureq::Response) -> Result<HttpResponse, HttpError> {
    let status = resp.status();
    let headers = extract_headers(&resp);
    let mut body = Vec::new();
    resp.into_reader()
        .read_to_end(&mut body)
        .map_err(HttpError::Io)?;

    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

/// Convert a ureq error-status response into our HttpResponse.
/// Unlike raw ureq, we return non-2xx as Ok so callers can inspect status.
fn response_from_ureq_with_status(
    status: u16,
    resp: ureq::Response,
) -> Result<HttpResponse, HttpError> {
    let headers = extract_headers(&resp);
    let mut body = Vec::new();
    resp.into_reader()
        .read_to_end(&mut body)
        .map_err(HttpError::Io)?;

    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

/// Extract all headers from a ureq response into a lowercase-keyed HashMap.
fn extract_headers(resp: &ureq::Response) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for name in resp.headers_names() {
        if let Some(value) = resp.header(&name) {
            headers.insert(name.to_lowercase(), value.to_string());
        }
    }
    headers
}
