// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workflow compiler as a WASI command, for a VM that grants it no
//! filesystem (M7.15). It compiles one request, over the workflow API its
//! host serves the run:
//!
//! 1. `GET /v1/run/input`: the [`CompileRequest`].
//! 2. `GET /v1/artifacts/{sha256}?offset=&length=`: each component, when the
//!    compile first reads it, in slices.
//! 3. `PUT /v1/artifacts/{sha256}?part=&parts=`: the composed `workflow.wasm`,
//!    in parts.
//!
//! (A Fenders node's host origin bounds a request at 4 MiB and a response at
//! 8 MiB, hence the slices and parts.)
//! 4. `POST /v1/run/outcome` (`completed`): a `GuestOutcome`
//!    (`runtara_workflows::compile_request`).
//!
//! It reaches the API at `http://api.runtara.internal`, or at
//! `RUNTARA_COMPILE_API` (for tests).

#[cfg(target_os = "wasi")]
use runtara_workflows::compile_request::{CompileRequest, GuestOutcome};

#[cfg(target_os = "wasi")]
mod http;

#[cfg(target_os = "wasi")]
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtara-compile-guest: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// The memory the compiler takes at once, before it compiles (see `run`).
#[cfg(target_os = "wasi")]
const PREGROW: usize = 192 * 1024 * 1024;

#[cfg(target_os = "wasi")]
fn run() -> Result<(), String> {
    let started = std::time::Instant::now();
    // Grow the linear memory once, now: a Fenders guest has no room reserved
    // for growth, so each growth moves the whole memory, and a compile that
    // grew step by step to its peak spent most of its time so. Freed, the
    // memory stays, and the compile's allocations reuse it.
    drop(Vec::<u8>::with_capacity(PREGROW));
    let fetching = std::sync::Arc::new(std::sync::Mutex::new((
        0usize,
        0usize,
        std::time::Duration::ZERO,
    )));
    let fetched = fetching.clone();
    let request: CompileRequest =
        serde_json::from_slice(&http::call("GET", "/v1/run/input", None)?)
            .map_err(|error| format!("the request: {error}"))?;
    let compiling = std::time::Instant::now();
    let outcome = match runtara_workflows::compile_request::compile(request, move |digest| {
        let at = std::time::Instant::now();
        let bytes = fetch(digest).map_err(std::io::Error::other)?;
        let mut fetched = fetched.lock().expect("fetch timings");
        *fetched = (
            fetched.0 + 1,
            fetched.1 + bytes.len(),
            fetched.2 + at.elapsed(),
        );
        Ok(bytes)
    }) {
        Ok((outcome, wasm)) => {
            let compiled = compiling.elapsed();
            let handing_back = std::time::Instant::now();
            let workflow_wasm = hand_back(&wasm)?;
            // On stderr, which a node keeps as the run's output: where a
            // compile's time went.
            let (files, bytes, fetch_time) = *fetching.lock().expect("fetch timings");
            eprintln!(
                "runtara-compile-guest: fetched {files} files ({bytes} bytes) in {} ms, compiled in {} ms (fetches included), handed back {} bytes in {} ms, {} ms in all",
                fetch_time.as_millis(),
                compiled.as_millis(),
                wasm.len(),
                handing_back.elapsed().as_millis(),
                started.elapsed().as_millis()
            );
            GuestOutcome::Compiled {
                outcome: Box::new(outcome),
                workflow_wasm,
            }
        }
        Err(failure) => GuestOutcome::Failed(failure),
    };
    let body = serde_json::to_vec(&outcome).map_err(|error| error.to_string())?;
    http::call("POST", "/v1/run/outcome", Some(body)).map(|_| ())
}

/// What one request or response carries: under the 4 MiB request and 8 MiB
/// response a Fenders node's host origin allows.
#[cfg(target_os = "wasi")]
const FETCH_SLICE: usize = 4 * 1024 * 1024;
#[cfg(target_os = "wasi")]
const HAND_BACK_PART: usize = 2 * 1024 * 1024;

/// The artifact `digest`, in slices: a slice shorter than asked for is its
/// last.
#[cfg(target_os = "wasi")]
fn fetch(digest: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    loop {
        let slice = http::call(
            "GET",
            &format!(
                "/v1/artifacts/{digest}?offset={}&length={FETCH_SLICE}",
                bytes.len()
            ),
            None,
        )?;
        let last = slice.len() < FETCH_SLICE;
        bytes.extend_from_slice(&slice);
        if last {
            return Ok(bytes);
        }
    }
}

/// Hands `wasm` back in parts, answering its SHA-256 once the host has it
/// whole.
#[cfg(target_os = "wasi")]
fn hand_back(wasm: &[u8]) -> Result<String, String> {
    use sha2::Digest;
    let digest: String = sha2::Sha256::digest(wasm)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let parts: Vec<&[u8]> = wasm.chunks(HAND_BACK_PART).collect();
    let mut whole = false;
    for (index, part) in parts.iter().enumerate() {
        let answer = http::call(
            "PUT",
            &format!("/v1/artifacts/{digest}?part={index}&parts={}", parts.len()),
            Some(part.to_vec()),
        )?;
        let answer: serde_json::Value = serde_json::from_slice(&answer)
            .map_err(|error| format!("the hand-back's answer: {error}"))?;
        whole = answer["whole"].as_bool().unwrap_or(false);
    }
    if !whole {
        return Err("the host did not take the composed component whole".into());
    }
    Ok(digest)
}

#[cfg(not(target_os = "wasi"))]
fn main() {
    eprintln!("runtara-compile-guest runs as a WASI command (wasm32-wasip2)");
    std::process::exit(2);
}
