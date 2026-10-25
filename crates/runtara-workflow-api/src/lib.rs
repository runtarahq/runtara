// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow API v1: the contract between a running workflow (or agent) and the
//! Runtara host.
//!
//! A workflow is a plain WASI component. It reaches durability, connections,
//! SQL, state and control over `wasi:http` at [`ORIGIN`]. The host routes that
//! origin to its router without a socket and attaches the run's [`Binding`];
//! the guest never names its tenant, instance or launch.
//!
//! This crate holds only shapes and pure helpers. It has no I/O and builds for
//! the host and for `wasm32-wasip2`, so the server and the guest SDK share one
//! definition of every header, error code and body.

mod binding;
mod connections;
mod error;
pub mod headers;
mod keys;
mod run;
#[cfg(feature = "server")]
pub mod server;
mod signals;
mod wake;

pub use binding::Binding;
pub use connections::{
    ConnectionTarget, OutboundDestination, OutboundFailure, OutboundMeta, SqlOperation,
};
pub use error::{ApiError, ErrorCode};
pub use keys::{IdempotencyKeyError, decode_path_key, encode_path_key, validate_idempotency_key};
pub use run::{ErrorInfo, OutcomeKind, PostedOutcome, RunInfo, Suspension};
pub use signals::{CustomSignalNotice, Event, InputRead, LifecycleNotice};
pub use wake::{SignalWait, Wake};

/// Scheme of the workflow API origin. There is no TLS: requests never touch a
/// socket.
pub const SCHEME: &str = "http";
/// Host of the workflow API origin. Never resolved; the host matches it before
/// any DNS or socket is involved. `.internal` is reserved for private use.
pub const HOST: &str = "api.runtara.internal";
/// The full origin a guest addresses.
pub const ORIGIN: &str = "http://api.runtara.internal";
/// Path prefix of every v1 endpoint.
pub const PATH_PREFIX: &str = "/v1";

/// Major version carried in the path.
pub const API_MAJOR: u32 = 1;
/// Highest minor version this build understands.
pub const API_MINOR: u32 = 0;

/// The value a guest sends in [`headers::API_VERSION`] for this build.
pub fn api_version() -> String {
    format!("{API_MAJOR}.{API_MINOR}")
}

/// Parse a `major.minor` version header value.
pub fn parse_api_version(value: &str) -> Option<(u32, u32)> {
    let (major, minor) = value.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Whether a host built against this crate serves a guest asking for
/// `requested`: same major, and a minor no newer than [`API_MINOR`].
// API_MINOR is 0 in the first release, which makes the bound look absurd.
#[allow(clippy::absurd_extreme_comparisons)]
pub fn serves_version(requested: (u32, u32)) -> bool {
    requested.0 == API_MAJOR && requested.1 <= API_MINOR
}

/// Default per-deployment caps (see the design's "Caps" table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub request_body_bytes: usize,
    pub response_body_bytes: usize,
    pub checkpoint_state_bytes: usize,
    pub continuation_bytes: usize,
    pub wakes_per_suspension: usize,
    pub wait_id_bytes: usize,
    pub event_batch_bytes: usize,
    pub event_batch_lines: usize,
    pub metadata_header_bytes: usize,
    pub long_poll_ms: u64,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            request_body_bytes: 16 * 1024 * 1024,
            response_body_bytes: 16 * 1024 * 1024,
            checkpoint_state_bytes: 16 * 1024 * 1024,
            continuation_bytes: 64 * 1024,
            wakes_per_suspension: 16,
            wait_id_bytes: 64,
            event_batch_bytes: 1024 * 1024,
            event_batch_lines: 1000,
            metadata_header_bytes: 32 * 1024,
            long_poll_ms: 30_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_scheme_and_host() {
        assert_eq!(ORIGIN, format!("{SCHEME}://{HOST}"));
    }

    #[test]
    fn version_negotiation() {
        assert_eq!(
            parse_api_version(&api_version()),
            Some((API_MAJOR, API_MINOR))
        );
        assert_eq!(parse_api_version(" 1.7 "), Some((1, 7)));
        assert_eq!(parse_api_version("1"), None);
        assert_eq!(parse_api_version("one.two"), None);
        assert!(serves_version((1, 0)));
        assert!(!serves_version((1, API_MINOR + 1)));
        assert!(!serves_version((2, 0)));
    }
}
