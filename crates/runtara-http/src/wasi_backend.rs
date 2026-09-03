// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WASI HTTP backend routed through Runtara's host-mediated transport.
//!
//! This module is only compiled when the `wasi` feature is enabled (and
//! the `native` feature is not). It implements the same public API as the
//! native ureq backend so that callers are unaware of the underlying transport.

use std::time::Duration;

use crate::{HttpError, HttpResponse, RequestBuilder};

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

/// Execute a request through `runtara:host-io/http`. The host owns the
/// absolute deadline and response-size policy, so `.call()` cannot become an
/// ungoverned alternative to `.call_agent()` for internal agents such as
/// Object Model.
pub(crate) fn execute(builder: RequestBuilder) -> Result<HttpResponse, HttpError> {
    crate::host_io::execute(builder)
}
