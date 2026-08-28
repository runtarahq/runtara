// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Guards the boundary this crate exists to hold: the host executes an agent
//! natively only when that agent genuinely cannot run in the wasm sandbox.
//!
//! Compression and XLSX used to have native workers here, on the belief that
//! `zip` and `calamine` could not target `wasm32-wasip2`. They can — `zip`'s
//! C-backed backends are optional features those capabilities never used, and
//! `calamine` is pure Rust — so both are ordinary WASM components now
//! (`crates/agents/runtara-agent-{compression,xlsx}`). SFTP is the only one
//! left, because libssh2 is a C library with no wasm target.
//!
//! A new entry in this set means someone added a native worker. That is a
//! deliberate architectural decision, not a routine change: it puts the agent
//! outside the sandbox and back on the "mostly sandboxed, except…" side of the
//! security story. Update this test consciously, or find the wasm path.

use std::collections::BTreeSet;

#[test]
fn sftp_is_the_only_natively_executed_agent() {
    let modules: BTreeSet<&str> = runtara_agents::registry::get_all_capabilities()
        .filter_map(|c| c.module)
        .collect();

    assert_eq!(
        modules,
        BTreeSet::from(["sftp"]),
        "host-executed agent set changed; see this file's header before updating"
    );
}

#[test]
fn wasm_backed_agents_are_not_executable_on_the_host() {
    // Their WASM components do the work in-guest and never call
    // /api/internal/agents, so the host must not offer a second implementation
    // that could silently diverge from the one workflows actually run.
    for (module, capability) in [
        ("compression", "create-archive"),
        ("compression", "extract-archive"),
        ("compression", "extract-file"),
        ("compression", "list-archive"),
        ("xlsx", "from-xlsx"),
        ("xlsx", "get-sheets"),
    ] {
        let result =
            runtara_agents::registry::execute_capability(module, capability, serde_json::json!({}));
        let err = result.expect_err(&format!(
            "{module}:{capability} must not execute on the host"
        ));
        assert!(
            err.contains("Unknown capability"),
            "{module}:{capability} failed for the wrong reason: {err}"
        );
    }
}
