// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The save-time validator as a WASI command for a guest placed on an
//! executor (docs/compiler-on-an-executor.md): it has no input or output of
//! its own there, and takes both over the workflow API its host serves the
//! run.
//!
//! 1. `GET /v1/run/input`: a `ClosureValidationRequest`, as JSON.
//! 2. `POST /v1/run/outcome` (`completed`): the `ClosureValidationReport`.
//!
//! It is the validation `runtara-validate-guest` does, which reads stdin and
//! writes stdout for a sandbox in the server's own process.

#[cfg(target_os = "wasi")]
#[path = "../http.rs"]
#[allow(dead_code)]
mod http;

#[cfg(target_os = "wasi")]
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtara-validate-placed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "wasi")]
fn run() -> Result<(), String> {
    let request: runtara_workflows::ClosureValidationRequest =
        serde_json::from_slice(&http::call("GET", "/v1/run/input", None)?)
            .map_err(|error| format!("the request: {error}"))?;
    let report = runtara_workflows::validate_workflow_closure(
        &request.root_workflow_id,
        &request.root,
        &request.catalog,
        &request.children,
    );
    let body = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
    http::call("POST", "/v1/run/outcome", Some(body)).map(|_| ())
}

#[cfg(not(target_os = "wasi"))]
fn main() {
    eprintln!("runtara-validate-placed runs as a WASI command (wasm32-wasip2)");
    std::process::exit(2);
}
