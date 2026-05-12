// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Browser/JS WASM HTTP backend placeholder.
//!
//! The public `runtara-http` API is synchronous. Browser `fetch` is async, so
//! this backend intentionally fails at call time. This keeps metadata and
//! validation-only WASM builds linkable without pretending agent HTTP execution
//! is available in the browser.

use std::time::Duration;

use crate::{HttpError, HttpResponse, RequestBuilder};

/// Browser/JS WASM HTTP client.
#[derive(Clone)]
pub struct WasmJsHttpClient;

impl WasmJsHttpClient {
    /// Create a browser/JS HTTP client.
    pub fn new() -> Self {
        Self
    }

    /// Create a browser/JS HTTP client with a timeout.
    pub fn with_timeout(_timeout: Duration) -> Self {
        Self
    }

    /// Create a request builder.
    pub fn request(&self, method: &str, url: &str) -> RequestBuilder {
        RequestBuilder {
            method: method.to_string(),
            url: url.to_string(),
            headers: Vec::new(),
            query_params: Vec::new(),
            body: None,
            timeout: None,
        }
    }
}

pub(crate) fn execute(_request: RequestBuilder) -> Result<HttpResponse, HttpError> {
    Err(HttpError::Transport(
        "HTTP execution is not available in browser validation WASM".to_string(),
    ))
}
