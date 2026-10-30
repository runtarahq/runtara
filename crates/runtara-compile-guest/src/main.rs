// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workflow compiler as a WASI command, for a VM that grants it no
//! filesystem (M7.15). It compiles one request, over the workflow API its
//! host serves the run:
//!
//! 1. `GET /v1/run/input`: the [`CompileRequest`].
//! 2. `GET /v1/artifacts/{sha256}`: each component, when the compile first
//!    reads it.
//! 3. `PUT /v1/artifacts`: the composed `workflow.wasm`.
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

#[cfg(target_os = "wasi")]
fn run() -> Result<(), String> {
    let request: CompileRequest =
        serde_json::from_slice(&http::call("GET", "/v1/run/input", None)?)
            .map_err(|error| format!("the request: {error}"))?;
    let outcome = match runtara_workflows::compile_request::compile(request, |digest| {
        http::call("GET", &format!("/v1/artifacts/{digest}"), None).map_err(std::io::Error::other)
    }) {
        Ok((outcome, wasm)) => {
            let uploaded = http::call("PUT", "/v1/artifacts", Some(wasm))?;
            let uploaded: serde_json::Value = serde_json::from_slice(&uploaded)
                .map_err(|error| format!("the upload's answer: {error}"))?;
            GuestOutcome::Compiled {
                outcome: Box::new(outcome),
                workflow_wasm: uploaded["sha256"].as_str().unwrap_or_default().to_owned(),
            }
        }
        Err(failure) => GuestOutcome::Failed(failure),
    };
    let body = serde_json::to_vec(&outcome).map_err(|error| error.to_string())?;
    http::call("POST", "/v1/run/outcome", Some(body)).map(|_| ())
}

#[cfg(not(target_os = "wasi"))]
fn main() {
    eprintln!("runtara-compile-guest runs as a WASI command (wasm32-wasip2)");
    std::process::exit(2);
}
