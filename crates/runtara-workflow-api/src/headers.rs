//! Header names of the workflow API. Lowercase, as HTTP/2 and `http` store them.

/// `major.minor` version the guest speaks.
pub const API_VERSION: &str = "runtara-api-version";
/// Required on every non-`GET` request; unique per launch.
pub const IDEMPOTENCY_KEY: &str = "idempotency-key";
/// `completed | failed | suspended` on `POST /v1/run/outcome`, and the task
/// outcome kind on `GET /v1/tasks/{id}/outcome`.
pub const OUTCOME: &str = "runtara-outcome";
/// The operation a call runs inside.
pub const OPERATION: &str = "runtara-operation";
/// JSON wakes of an operation suspension.
pub const WAKES: &str = "runtara-wakes";
/// JSON announcement of a pending lifecycle signal, piggybacked on checkpoints.
pub const PENDING_SIGNAL: &str = "runtara-pending-signal";
/// JSON announcement of a pending custom signal, piggybacked on checkpoints.
pub const CUSTOM_SIGNAL: &str = "runtara-custom-signal";
/// JSON metadata of a delivered lifecycle signal, whose body is its payload.
pub const SIGNAL: &str = "runtara-signal";
/// JSON metadata of an outbound HTTP request.
pub const OUTBOUND: &str = "runtara-outbound";
/// Upstream status of a proxied HTTP response.
pub const UPSTREAM_STATUS: &str = "runtara-upstream-status";
/// JSON header pairs of a proxied HTTP response.
pub const UPSTREAM_HEADERS: &str = "runtara-upstream-headers";
/// JSON error of a failed outbound call, whose body is the upstream body.
pub const ERROR: &str = "runtara-error";
/// JSON metadata of a child-run start.
pub const TASK: &str = "runtara-task";

/// Prefix of headers only the host may set. Stripped from guest requests.
pub const BINDING_PREFIX: &str = "runtara-binding-";

/// Headers a guest may never send to the workflow API, in addition to any
/// [`BINDING_PREFIX`] header: credentials, cookies, proxy metadata and
/// hop-by-hop headers.
pub const STRIPPED: &[&str] = &[
    "authorization",
    "cookie",
    "forwarded",
    "via",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Whether a guest-supplied header must be removed before routing.
pub fn is_stripped(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with(BINDING_PREFIX)
        || name.starts_with("x-forwarded-")
        || STRIPPED.contains(&name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_headers_are_stripped() {
        for name in [
            "Authorization",
            "cookie",
            "X-Forwarded-For",
            "runtara-binding-tenant",
            "Transfer-Encoding",
        ] {
            assert!(is_stripped(name), "{name}");
        }
        for name in [API_VERSION, IDEMPOTENCY_KEY, OUTCOME, "content-type"] {
            assert!(!is_stripped(name), "{name}");
        }
    }
}
