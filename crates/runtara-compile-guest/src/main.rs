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
//! Given `{"worker": {"idle_ms", "until_unix_ms", "deflate"}}` as its input instead, it
//! is its tenant's warm compiler (SYN-930): it takes compiles one after
//! another from `GET /v1/compiles/next`, the same calls under
//! `/v1/compiles/{id}/`, fetching each component once for all of them.
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
    // Its host trusts nothing it makes for being valid, and whatever runs a
    // component validates it: validating here only cost the compile.
    runtara_workflows::direct_wasm::set_composition_validated(false);
    // Grow the linear memory once, now: a Fenders guest has no room reserved
    // for growth, so each growth moves the whole memory, and a compile that
    // grew step by step to its peak spent most of its time so. Freed, the
    // memory stays, and the compile's allocations reuse it.
    drop(Vec::<u8>::with_capacity(PREGROW));
    let input: serde_json::Value =
        serde_json::from_slice(&http::call("GET", "/v1/run/input", None)?)
            .map_err(|error| format!("the input: {error}"))?;
    if let Some(settings) = input.get("worker") {
        return work(settings);
    }
    let request: CompileRequest =
        serde_json::from_value(input).map_err(|error| format!("the request: {error}"))?;
    let outcome = compile(request, "/v1/artifacts", &Components::default(), false)?;
    let body = serde_json::to_vec(&outcome).map_err(|error| error.to_string())?;
    http::call("POST", "/v1/run/outcome", Some(body)).map(|_| ())
}

/// The components fetched so far, by SHA-256: a warm compiler fetches each
/// once for all its compiles, and a digest names its bytes for good.
#[cfg(target_os = "wasi")]
type Components = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>;

/// A warm compiler (SYN-930): takes its tenant's compiles one after another
/// (`GET /v1/compiles/next`) until none has come for `idle_ms`, or until
/// `until_unix_ms`, when it stops taking them. A failure to reach the API
/// ends it, so a fresh one takes its place.
#[cfg(target_os = "wasi")]
fn work(settings: &serde_json::Value) -> Result<(), String> {
    let idle = std::time::Duration::from_millis(settings["idle_ms"].as_u64().unwrap_or(300_000));
    let until = settings["until_unix_ms"].as_u64().unwrap_or(u64::MAX);
    let deflate = settings["deflate"].as_bool().unwrap_or(false);
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    };
    let components = Components::default();
    let mut last = std::time::Instant::now();
    loop {
        let left = until.saturating_sub(now_ms());
        let waiting = idle.saturating_sub(last.elapsed());
        if left == 0 || waiting.is_zero() {
            return Ok(());
        }
        let wait = waiting.as_millis().min(u128::from(left)).min(25_000);
        let next = http::call("GET", &format!("/v1/compiles/next?wait_ms={wait}"), None)?;
        if next.is_empty() {
            continue;
        }
        let next: serde_json::Value =
            serde_json::from_slice(&next).map_err(|error| format!("the next compile: {error}"))?;
        let id = next["id"]
            .as_str()
            .ok_or("the next compile has no id")?
            .to_owned();
        let request: CompileRequest = serde_json::from_value(next["request"].clone())
            .map_err(|error| format!("the request: {error}"))?;
        let (_, waited) = http::take_timings();
        let outcome = compile(
            request,
            &format!("/v1/compiles/{id}/artifacts"),
            &components,
            deflate,
        )?;
        let body = serde_json::to_vec(&outcome).map_err(|error| error.to_string())?;
        http::call("POST", &format!("/v1/compiles/{id}/outcome"), Some(body))?;
        let (calls, took) = http::take_timings();
        eprintln!(
            "runtara-compile-guest: compile {id}: waited {} ms for it, {calls} calls in {} ms",
            waited.as_millis(),
            took.as_millis()
        );
        last = std::time::Instant::now();
    }
}

/// Compiles `request`, fetching each component it reads and `components`
/// lacks from `artifacts` (`{artifacts}/{sha256}`), and hands the composed
/// component back there: what it made, or why it failed.
#[cfg(target_os = "wasi")]
fn compile(
    request: CompileRequest,
    artifacts: &str,
    components: &Components,
    deflate: bool,
) -> Result<GuestOutcome, String> {
    let started = std::time::Instant::now();
    let fetching = std::sync::Arc::new(std::sync::Mutex::new((
        0usize,
        0usize,
        std::time::Duration::ZERO,
    )));
    let outcome = {
        let (fetching, components, artifacts) =
            (fetching.clone(), components.clone(), artifacts.to_owned());
        runtara_workflows::compile_request::compile(request, move |digest| {
            if let Some(bytes) = components.lock().expect("components").get(digest) {
                return Ok(bytes.clone());
            }
            let at = std::time::Instant::now();
            let bytes = fetch(&artifacts, digest).map_err(std::io::Error::other)?;
            let mut fetched = fetching.lock().expect("fetch timings");
            *fetched = (
                fetched.0 + 1,
                fetched.1 + bytes.len(),
                fetched.2 + at.elapsed(),
            );
            components
                .lock()
                .expect("components")
                .insert(digest.to_owned(), bytes.clone());
            Ok(bytes)
        })
    };
    Ok(match outcome {
        Ok((outcome, wasm)) => {
            let compiled = started.elapsed();
            let handing_back = std::time::Instant::now();
            let workflow_wasm = hand_back(artifacts, &wasm, deflate)?;
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
    })
}

/// What one request or response carries: under the 4 MiB request and 8 MiB
/// response a Fenders node's host origin allows.
#[cfg(target_os = "wasi")]
const FETCH_SLICE: usize = 4 * 1024 * 1024;
/// A hand-back part: as large as a request may be, less room for its
/// headers, as each part is a round trip.
#[cfg(target_os = "wasi")]
const HAND_BACK_PART: usize = 3 * 1024 * 1024 + 768 * 1024;

/// The artifact `digest` at `artifacts`, in slices: a slice shorter than
/// asked for is its last.
#[cfg(target_os = "wasi")]
fn fetch(artifacts: &str, digest: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    loop {
        let slice = http::call(
            "GET",
            &format!(
                "{artifacts}/{digest}?offset={}&length={FETCH_SLICE}",
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

/// Hands `wasm` back to `artifacts` in parts, answering its SHA-256 once the
/// host has it whole, deflated when `deflate`: a component deflates to about
/// a third, which pays where the link to the host is slow (SYN-930).
#[cfg(target_os = "wasi")]
fn hand_back(artifacts: &str, wasm: &[u8], deflate: bool) -> Result<String, String> {
    use sha2::Digest;
    let digest: String = sha2::Sha256::digest(wasm)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let sent = if deflate {
        use std::io::Write;
        let mut encoder = flate2::write::DeflateEncoder::new(
            Vec::with_capacity(wasm.len() / 2),
            flate2::Compression::fast(),
        );
        encoder.write_all(wasm).map_err(|error| error.to_string())?;
        encoder.finish().map_err(|error| error.to_string())?
    } else {
        wasm.to_vec()
    };
    let encoding = if deflate { "&encoding=deflate" } else { "" };
    let parts: Vec<&[u8]> = sent.chunks(HAND_BACK_PART).collect();
    let mut whole = false;
    for (index, part) in parts.iter().enumerate() {
        let answer = http::call(
            "PUT",
            &format!(
                "{artifacts}/{digest}?part={index}&parts={}{encoding}",
                parts.len()
            ),
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
