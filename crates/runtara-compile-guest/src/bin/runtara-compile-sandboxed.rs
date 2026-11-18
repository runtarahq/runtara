// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workflow compiler as a WASI command for a sandbox in the engine's own
//! process (docs/splitting-the-engine.md, Phase 6): a Wasmtime store with no
//! network and no environment, a bound on its memory and its time, and of
//! the filesystem only the component files the compile may read, read-only.
//!
//! It compiles one request and ends:
//!
//! 1. stdin: the [`CompileRequest`], as JSON.
//! 2. `/components/{name}` and `/extra/{n}/{name}`: the files the request
//!    names, where the compiler sees them. Each is checked against the
//!    SHA-256 the request gives it.
//! 3. stdout: eight bytes, the length of what follows them as a
//!    little-endian number; a [`GuestOutcome`] as JSON, of that length; and,
//!    for a compile that made one, the composed component.
//!
//! What it writes is input to whoever started it: the host checks the
//! component against the outcome and takes nothing of either on its word.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;

use runtara_workflows::compile_request::{CompileRequest, GuestOutcome};
use sha2::Digest;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtara-compile-sandboxed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("reading the request: {error}"))?;
    let request: CompileRequest =
        serde_json::from_slice(&input).map_err(|error| format!("the request: {error}"))?;
    drop(input);

    // Each file the request names, by its SHA-256: where the host put it.
    let mut by_digest = BTreeMap::new();
    for (name, digest) in &request.components {
        by_digest.insert(digest.clone(), PathBuf::from("/components").join(name));
    }
    for (index, files) in request.extra_components.iter().enumerate() {
        for (name, digest) in files {
            by_digest.insert(
                digest.clone(),
                PathBuf::from("/extra").join(index.to_string()).join(name),
            );
        }
    }
    let fetch = move |digest: &str| -> std::io::Result<Vec<u8>> {
        let path = by_digest
            .get(digest)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, digest.to_owned()))?;
        std::fs::read(path)
    };

    let (outcome, made) = match runtara_workflows::compile_request::compile(request, fetch) {
        Ok((outcome, made)) => {
            let workflow_wasm = sha2::Sha256::digest(&made)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            (
                GuestOutcome::Compiled {
                    outcome: Box::new(outcome),
                    workflow_wasm,
                },
                made,
            )
        }
        Err(failure) => (GuestOutcome::Failed(failure), Vec::new()),
    };
    let outcome = serde_json::to_vec(&outcome).map_err(|error| format!("the outcome: {error}"))?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&(outcome.len() as u64).to_le_bytes())
        .and_then(|()| stdout.write_all(&outcome))
        .and_then(|()| stdout.write_all(&made))
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("writing the outcome: {error}"))
}
