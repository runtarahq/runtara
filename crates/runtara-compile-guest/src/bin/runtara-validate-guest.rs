// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The save-time validator as a WASI command (M7.15), for a server to run on
//! a tenant's workflow in a sandbox of its own (a Wasmtime store with fuel
//! and memory limits) rather than natively: it reads a
//! `ClosureValidationRequest` as JSON on stdin and writes the
//! `ClosureValidationReport` as JSON on stdout.

use std::io::{Read, Write};

use runtara_workflows::ClosureValidationRequest;

fn main() -> std::process::ExitCode {
    let mut input = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("runtara-validate-guest: reading the request: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let request: ClosureValidationRequest = match serde_json::from_slice(&input) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("runtara-validate-guest: the request: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let report = runtara_workflows::validate_workflow_closure(
        &request.root_workflow_id,
        &request.root,
        &request.catalog,
        &request.children,
    );
    let output = serde_json::to_vec(&report).expect("a report serializes");
    match std::io::stdout().write_all(&output) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("runtara-validate-guest: writing the report: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
