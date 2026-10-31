// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Blocking workflow API calls over `wasi:http` 0.2: the compile that makes
//! them is synchronous.

use wasi::http::outgoing_handler;
use wasi::http::types::{Fields, Method, OutgoingBody, OutgoingRequest, Scheme};

static NEXT_KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// `method` `path` on the workflow API, with `body`: answers the response's
/// body, or why the call failed (a status other than 2xx among them).
/// The calls made since [`take_timings`] was last asked, and the time they took.
static TIMINGS: std::sync::Mutex<(u32, std::time::Duration)> =
    std::sync::Mutex::new((0, std::time::Duration::ZERO));

/// The calls made since last asked, and the time they took.
pub(crate) fn take_timings() -> (u32, std::time::Duration) {
    std::mem::take(&mut *TIMINGS.lock().expect("timings"))
}

pub(crate) fn call(method: &str, path: &str, body: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let started = std::time::Instant::now();
    let answer = call_once(method, path, body);
    let mut timings = TIMINGS.lock().expect("timings");
    timings.0 += 1;
    timings.1 += started.elapsed();
    answer
}

fn call_once(method: &str, path: &str, body: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let (scheme, authority) = match std::env::var("RUNTARA_COMPILE_API") {
        Ok(base) => {
            let (scheme, authority) = base
                .split_once("://")
                .ok_or_else(|| format!("RUNTARA_COMPILE_API {base:?} names no scheme"))?;
            (
                if scheme == "https" {
                    Scheme::Https
                } else {
                    Scheme::Http
                },
                authority.trim_end_matches('/').to_owned(),
            )
        }
        Err(_) => (Scheme::Http, runtara_workflow_api::HOST.to_owned()),
    };
    let headers = Fields::new();
    if method != "GET" {
        // Writes are idempotent by key, as the SDK's are.
        let key = format!(
            "compile-{}",
            NEXT_KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        headers
            .append(
                runtara_workflow_api::headers::IDEMPOTENCY_KEY,
                &key.into_bytes(),
            )
            .map_err(|error| format!("{error:?}"))?;
    }
    if let Some(body) = &body {
        headers
            .append("Content-Length", &body.len().to_string().into_bytes())
            .map_err(|error| format!("{error:?}"))?;
    }
    if path == "/v1/run/outcome" {
        headers
            .append("Runtara-Outcome", b"completed".as_ref())
            .map_err(|error| format!("{error:?}"))?;
    }
    let request = OutgoingRequest::new(headers);
    let method = match method {
        "GET" => Method::Get,
        "PUT" => Method::Put,
        _ => Method::Post,
    };
    request
        .set_method(&method)
        .and_then(|()| request.set_scheme(Some(&scheme)))
        .and_then(|()| request.set_authority(Some(&authority)))
        .and_then(|()| request.set_path_with_query(Some(path)))
        .map_err(|()| format!("{path}: an invalid request"))?;
    let outgoing = request.body().map_err(|()| "request body".to_owned())?;
    let future =
        outgoing_handler::handle(request, None).map_err(|error| format!("{path}: {error:?}"))?;
    if let Some(body) = body {
        let stream = outgoing.write().map_err(|()| "request stream".to_owned())?;
        for chunk in body.chunks(4096) {
            stream
                .blocking_write_and_flush(chunk)
                .map_err(|error| format!("{path}: writing: {error:?}"))?;
        }
        drop(stream);
    }
    OutgoingBody::finish(outgoing, None).map_err(|error| format!("{path}: {error:?}"))?;
    future.subscribe().block();
    let response = future
        .get()
        .ok_or_else(|| format!("{path}: no response"))?
        .map_err(|()| format!("{path}: no response"))?
        .map_err(|error| format!("{path}: {error:?}"))?;
    let status = response.status();
    let incoming = response
        .consume()
        .map_err(|()| format!("{path}: no body"))?;
    let stream = incoming.stream().map_err(|()| format!("{path}: no body"))?;
    let mut bytes = Vec::new();
    loop {
        match stream.blocking_read(64 * 1024) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(error) => return Err(format!("{path}: reading: {error:?}")),
        }
    }
    if !(200..300).contains(&status) {
        return Err(format!(
            "{path}: {status}: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(300)])
        ));
    }
    Ok(bytes)
}
