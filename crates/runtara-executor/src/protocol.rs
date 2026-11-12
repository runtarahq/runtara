// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The placement API: what the server asks of an executor, over mutual TLS.
//!
//! | Call | Purpose |
//! |---|---|
//! | `HEAD /v1/artifacts/{tenant}/{sha256}` | whether the workflow is staged |
//! | `PUT /v1/artifacts/{tenant}/{sha256}` | stage it: the component's bytes, checked against the digest and compiled |
//! | `POST /v1/runs` | place a run: [`Place`] → [`Placed`] |
//! | `GET /v1/runs/{id}?wait_ms=` | the run's [`RunState`], waiting up to `wait_ms` for it to end |
//! | `DELETE /v1/runs/{id}` | stop it |
//!
//! A refusal is a [`Refusal`] with a 4xx or 5xx status.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The run protocol a spec speaks (`runtara-environment`'s `RUN_PROTOCOL`).
pub const RUN_PROTOCOL: u64 = 1;
/// The largest component staged.
pub const MAX_ARTIFACT_BYTES: usize = 128 * 1024 * 1024;
/// The longest a status call is held open.
pub const MAX_WAIT_MS: u64 = 30_000;

/// The name in the server's certificate. Connections are verified against
/// these names, not against the hosts they were reached at, so a deployment
/// names its hosts as it likes.
pub const SERVER_NAME: &str = "runtara-server";
/// The name in the executor's certificate.
pub const EXECUTOR_NAME: &str = "runtara-executor";

/// A tenant as a path segment: 1 to 128 of `[A-Za-z0-9_-]`.
pub fn valid_tenant(tenant: &str) -> bool {
    (1..=128).contains(&tenant.len())
        && tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// A SHA-256 as lowercase hex.
pub fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `POST /v1/runs`.
#[derive(Serialize, Deserialize)]
pub struct Place {
    pub tenant: String,
    /// The staged workflow's digest.
    pub artifact: String,
    pub spec: RunSpec,
}

/// A placed run.
#[derive(Debug, Serialize, Deserialize)]
pub struct Placed {
    pub run: String,
}

/// A run as its executor sees it. What the run's outcome is, the run posted
/// to the server; this is what became of its process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// Its process exited with this code.
    Exited {
        code: i64,
    },
    /// It was killed, stopped, or could not be started.
    Failed {
        reason: String,
    },
}

/// Why a call was refused.
#[derive(Debug, Serialize, Deserialize)]
pub struct Refusal {
    /// `invalid`, `not_found`, `capacity`, `compile` or `internal`.
    pub error: String,
    pub message: String,
}

/// A run's spec: what the guest may use, and the run protocol's fields, which
/// `runtara-environment`'s `RunChannel::attach` adds. It holds the launch's
/// credential, so it is never logged and has no `Debug`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    pub protocol: u64,
    pub run: RunIdentity,
    pub channel: Channel,
    /// The guest's linear memory, summed over its memories.
    pub wasm_memory_bytes: u64,
    /// How long the guest may run.
    pub lifetime_seconds: u64,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Which launch a run is.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIdentity {
    pub execution: String,
    pub generation: u64,
    #[serde(default)]
    pub trace_id: Option<String>,
}

/// Where a run's workflow API calls go, and as whom.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    /// `https://host:port`.
    pub endpoint: String,
    /// The launch's bearer credential.
    pub credential: String,
    pub trust: Trust,
}

/// How the run channel's endpoint is verified.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trust {
    /// The only roots trusted for it.
    pub ca_pem: String,
    /// The SHA-256 of its certificate's public key, lowercase hex.
    #[serde(default)]
    pub pin: Option<String>,
    /// The name its certificate is for, when not the endpoint's host.
    #[serde(default)]
    pub server_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_is_a_plain_path_segment() {
        assert!(valid_tenant("org_1-a"));
        assert!(!valid_tenant(""));
        assert!(!valid_tenant("a/b"));
        assert!(!valid_tenant(".."));
        assert!(!valid_tenant(&"a".repeat(129)));
    }

    #[test]
    fn a_digest_is_lowercase_hex() {
        assert!(valid_digest(&"0a".repeat(32)));
        assert!(!valid_digest(&"0A".repeat(32)));
        assert!(!valid_digest("0a"));
    }

    #[test]
    fn a_spec_with_a_field_it_does_not_know_is_refused() {
        let spec = serde_json::json!({
            "protocol": 1,
            "run": {"execution": "l", "generation": 1},
            "channel": {"endpoint": "https://s", "credential": "c", "trust": {"ca_pem": ""}},
            "wasm_memory_bytes": 1, "lifetime_seconds": 1, "tcp_allow": ["1.2.3.4:5"],
        });
        assert!(serde_json::from_value::<RunSpec>(spec).is_err());
    }
}
