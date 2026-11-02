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
//! Given `{"worker": {"idle_ms", "until_unix_ms", "deflate", "splice", "together"}}` as its input instead, it
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
    let (outcome, _) = compile(
        request,
        "/v1/artifacts",
        &Components::default(),
        HandBack::default(),
    )?;
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
    // A host that rebuilds a splice from the components it served (see
    // `hand_back`) says so; one that does not is handed back the bytes.
    let splice = settings["splice"].as_bool().unwrap_or(false);
    // A host that takes a splice with the outcome says so: one call, not two.
    let together = settings["together"].as_bool().unwrap_or(false);
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
        let (outcome, held) = compile(
            request,
            &format!("/v1/compiles/{id}/artifacts"),
            &components,
            HandBack {
                deflate,
                splice,
                together,
            },
        )?;
        let body = serde_json::to_vec(&outcome).map_err(|error| error.to_string())?;
        match held {
            // The splice and the outcome, framed: the outcome's length, the
            // outcome, then the splice.
            Some((digest, splice)) => {
                let mut framed = (body.len() as u32).to_le_bytes().to_vec();
                framed.extend(&body);
                framed.extend(splice);
                // Deflated where the link is slow: the code and the outcome
                // shrink to about a fifth, so the call needs fewer round
                // trips on a fresh connection.
                let (framed, compressed) = if deflate {
                    use std::io::Write;
                    let mut encoder = flate2::write::DeflateEncoder::new(
                        Vec::with_capacity(framed.len() / 3),
                        flate2::Compression::fast(),
                    );
                    encoder
                        .write_all(&framed)
                        .map_err(|error| error.to_string())?;
                    (
                        encoder.finish().map_err(|error| error.to_string())?,
                        "&compressed=deflate",
                    )
                } else {
                    (framed, "")
                };
                http::call(
                    "POST",
                    &format!(
                        "/v1/compiles/{id}/outcome?artifact={digest}&encoding=splice{compressed}"
                    ),
                    Some(framed),
                )?;
            }
            None => {
                http::call("POST", &format!("/v1/compiles/{id}/outcome"), Some(body))?;
            }
        }
        let (calls, took) = http::take_timings();
        eprintln!(
            "runtara-compile-guest: compile {id}: waited {} ms for it, {calls} calls in {} ms",
            waited.as_millis(),
            took.as_millis()
        );
        last = std::time::Instant::now();
    }
}

/// How a compile hands back what it made: deflated, or as a splice of the
/// components it was given.
#[cfg(target_os = "wasi")]
#[derive(Clone, Copy, Default)]
struct HandBack {
    deflate: bool,
    splice: bool,
    /// The host takes a splice with the outcome, in one call.
    together: bool,
}

/// A splice held back to go with the outcome: its component's SHA-256, and
/// the splice.
#[cfg(target_os = "wasi")]
type Held = Option<(String, Vec<u8>)>;

/// Compiles `request`, fetching each component it reads and `components`
/// lacks from `artifacts` (`{artifacts}/{sha256}`), and hands the composed
/// component back there: what it made, or why it failed. A splice the host
/// takes with the outcome is held back for it instead.
#[cfg(target_os = "wasi")]
fn compile(
    request: CompileRequest,
    artifacts: &str,
    components: &Components,
    how: HandBack,
) -> Result<(GuestOutcome, Held), String> {
    // What a splice may name: this compile's own components.
    let given: Vec<String> = request
        .components
        .values()
        .chain(
            request
                .extra_components
                .iter()
                .flat_map(|files| files.values()),
        )
        .cloned()
        .collect();
    let started = std::time::Instant::now();
    let fetching = std::sync::Arc::new(std::sync::Mutex::new((
        0usize,
        0usize,
        std::time::Duration::ZERO,
    )));
    let outcome = {
        let (fetching, components, artifacts) =
            (fetching.clone(), components.clone(), artifacts.to_owned());
        // Each file is checked against its SHA-256 once, when fetched, and
        // kept: the compile need not check it again.
        runtara_workflows::compile_request::compile_verified(request, move |digest| {
            if let Some(bytes) = components.lock().expect("components").get(digest) {
                return Ok(bytes.clone());
            }
            let at = std::time::Instant::now();
            let bytes = fetch(&artifacts, digest).map_err(std::io::Error::other)?;
            if sha256_hex(&bytes) != digest {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{digest} is not the file it was named for"),
                ));
            }
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
            // Its SHA-256 as the compile computed it: hashing 6 MB again in
            // WASM took much of the hand-back.
            let placements = if how.splice {
                placements(&wasm, &given, components)
            } else {
                Vec::new()
            };
            let (workflow_wasm, sent, encoding, held) =
                hand_back(artifacts, &wasm, &outcome.binary_checksum, how, &placements)?;
            // On stderr, which a node keeps as the run's output: where a
            // compile's time went.
            let (files, bytes, fetch_time) = *fetching.lock().expect("fetch timings");
            eprintln!(
                "runtara-compile-guest: fetched {files} files ({bytes} bytes) in {} ms, compiled in {} ms (fetches included), handed back {} bytes as {sent} ({encoding}, {} components placed) in {} ms, {} ms in all",
                fetch_time.as_millis(),
                compiled.as_millis(),
                wasm.len(),
                placements.len(),
                handing_back.elapsed().as_millis(),
                started.elapsed().as_millis()
            );
            (
                GuestOutcome::Compiled {
                    outcome: Box::new(outcome),
                    workflow_wasm,
                },
                held,
            )
        }
        Err(failure) => (GuestOutcome::Failed(failure), None),
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

/// The least of a component worth naming in a splice rather than sending.
#[cfg(target_os = "wasi")]
const PLACED_MIN: usize = 64 * 1024;

/// Where each of `given` (this compile's components) that `components` holds
/// lies in `wasm` as it is. Composition nests each component it was given as
/// a component section of the composed one, byte for byte, so a walk of its
/// sections finds them, matching a section to a component of its length.
#[cfg(target_os = "wasi")]
fn placements(
    wasm: &[u8],
    given: &[String],
    components: &Components,
) -> Vec<runtara_workflow_api::splice::Placement> {
    /// A component section's id.
    const COMPONENT_SECTION: u8 = 4;
    let components = components.lock().expect("components");
    let mut by_length: std::collections::HashMap<usize, Vec<&String>> =
        std::collections::HashMap::new();
    for sha256 in given {
        if let Some(bytes) = components
            .get(sha256)
            .filter(|bytes| bytes.len() >= PLACED_MIN)
        {
            by_length.entry(bytes.len()).or_default().push(sha256);
        }
    }
    let mut placed = Vec::new();
    // Past the preamble: the magic and the version and layer.
    let mut at = 8;
    while let Some(&id) = wasm.get(at) {
        let Some((size, payload)) = leb128_u32(wasm, at + 1) else {
            break;
        };
        let Some(end) = payload
            .checked_add(size as usize)
            .filter(|end| *end <= wasm.len())
        else {
            break;
        };
        if id == COMPONENT_SECTION
            && let Some(sha256) = by_length.get(&(size as usize)).and_then(|candidates| {
                candidates
                    .iter()
                    .find(|sha256| components[sha256.as_str()].as_slice() == &wasm[payload..end])
            })
        {
            placed.push(runtara_workflow_api::splice::Placement {
                sha256: (*sha256).clone(),
                at: payload,
                length: size as usize,
            });
        }
        at = end;
    }
    placed
}

/// The unsigned LEB128 at `at` in `bytes`, and where what follows it starts.
#[cfg(target_os = "wasi")]
fn leb128_u32(bytes: &[u8], mut at: usize) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(at)?;
        at += 1;
        value |= u32::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some((value, at));
        }
    }
    None
}

/// Hands `wasm`, whose SHA-256 is `digest`, back to `artifacts` in parts,
/// answering its SHA-256 once the host has it whole (checking it is what the
/// parts make), with how many bytes crossed and as what. With `placements`, as a splice: its own bytes, and where the
/// components it was given lie in it, which the host has. Else deflated when
/// `how` says: a component deflates to about a third, which pays where the
/// link to the host is slow (SYN-930).
#[cfg(target_os = "wasi")]
fn hand_back(
    artifacts: &str,
    wasm: &[u8],
    digest: &str,
    how: HandBack,
    placements: &[runtara_workflow_api::splice::Placement],
) -> Result<(String, usize, &'static str, Held), String> {
    let digest = digest.to_owned();
    let spliced = !placements.is_empty();
    let deflate = how.deflate && !spliced;
    let sent = if spliced {
        runtara_workflow_api::splice::encode(wasm, placements)
    } else if deflate {
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
    // A splice is a small part of what it rebuilds: with the outcome, it
    // costs no call of its own.
    if spliced && how.together && sent.len() <= HAND_BACK_PART {
        let len = sent.len();
        return Ok((
            digest.clone(),
            len,
            "splice, with the outcome",
            Some((digest, sent)),
        ));
    }
    let encoding = if spliced {
        "&encoding=splice"
    } else if deflate {
        "&encoding=deflate"
    } else {
        ""
    };
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
    let encoding = if spliced {
        "splice"
    } else if deflate {
        "deflate"
    } else {
        "raw"
    };
    Ok((digest, sent.len(), encoding, None))
}

#[cfg(target_os = "wasi")]
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(not(target_os = "wasi"))]
fn main() {
    eprintln!("runtara-compile-guest runs as a WASI command (wasm32-wasip2)");
    std::process::exit(2);
}
