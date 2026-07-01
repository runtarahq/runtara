//! Startup helpers that guard against the single worst misconfiguration in the
//! non-OIDC auth modes: binding the public listener to a non-loopback address while
//! in-process authentication is disabled.

use std::net::IpAddr;

use crate::auth::AuthProviderKind;

/// Returns true if `host` resolves to a loopback address. Accepts `127.0.0.0/8`, `::1`,
/// and the literal string `"localhost"`; everything else (including `0.0.0.0` and `::`)
/// is considered non-loopback.
pub fn is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(addr) => addr.is_loopback(),
        Err(_) => false,
    }
}

/// Refuse to boot in a provider mode that disables in-process auth if the public
/// listener would accept connections from anywhere but the local machine.
///
/// Returns the expected operator-facing error message; callers print it and exit with
/// a non-zero status (matching the `Valkey` validation pattern in `server::start`).
pub fn enforce_loopback_for_unauthenticated(
    kind: AuthProviderKind,
    host: &str,
) -> Result<(), String> {
    if !kind.requires_loopback() {
        return Ok(());
    }
    if is_loopback(host) {
        return Ok(());
    }
    Err(format!(
        "AUTH_PROVIDER={} requires SERVER_HOST to be a loopback address \
         (127.0.0.1, ::1, or localhost); got '{}'. \
         Unauthenticated modes must not accept non-local connections — bind RUNTARA \
         to loopback and put a reverse proxy in front of it. \
         See docs/deployment/auth-modes.md and docs/reference/proxy/.",
        kind.as_str(),
        host,
    ))
}

/// Refuse to boot the **internal** listener on a non-loopback host unless a
/// shared-secret authenticator is configured for the internal routes.
///
/// Unlike the public listener, the internal API (`/api/internal/proxy`,
/// `/api/internal/agents`, internal object-model) is *always* unauthenticated
/// and derives the tenant from an `X-Org-Id` header. It also injects connection
/// credentials server-side and performs outbound egress, so exposing it on a
/// non-loopback bind would let any host that can send an `X-Org-Id` header drive
/// credentialed egress and SSRF. This guard is therefore independent of
/// [`AuthProviderKind`] — the public auth provider does not protect this port.
///
/// Returns the operator-facing error message; callers print it and exit non-zero.
pub fn enforce_internal_listener_safe(
    internal_host: &str,
    has_internal_secret: bool,
) -> Result<(), String> {
    if is_loopback(internal_host) {
        return Ok(());
    }
    if has_internal_secret {
        return Ok(());
    }
    Err(format!(
        "INTERNAL_HOST='{internal_host}' is not a loopback address and no \
         RUNTARA_INTERNAL_SHARED_SECRET is configured. The internal API \
         (/api/internal/proxy, /api/internal/agents, internal object-model) is \
         UNAUTHENTICATED and injects connection credentials server-side; exposing \
         it on a non-loopback bind would let any host that can send an X-Org-Id \
         header drive credentialed egress and SSRF. Bind INTERNAL_HOST to a \
         loopback address (127.0.0.1, ::1, localhost), or set \
         RUNTARA_INTERNAL_SHARED_SECRET and front the internal port with mTLS. \
         See docs/reference/proxy/."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_listener_guard() {
        // Loopback never needs a secret.
        assert!(enforce_internal_listener_safe("127.0.0.1", false).is_ok());
        assert!(enforce_internal_listener_safe("::1", false).is_ok());
        assert!(enforce_internal_listener_safe("localhost", false).is_ok());
        assert!(enforce_internal_listener_safe("127.0.0.42", false).is_ok());

        // Non-loopback without a secret refuses to boot.
        for host in ["0.0.0.0", "::", "10.0.0.5"] {
            let err = enforce_internal_listener_safe(host, false)
                .expect_err("non-loopback internal bind must be rejected without a secret");
            assert!(err.contains(host));
            assert!(err.contains("RUNTARA_INTERNAL_SHARED_SECRET"));
        }

        // Non-loopback WITH a configured secret is allowed (documented escape hatch).
        assert!(enforce_internal_listener_safe("0.0.0.0", true).is_ok());
        assert!(enforce_internal_listener_safe("10.0.0.5", true).is_ok());
    }

    #[test]
    fn internal_guard_independent_of_auth_kind() {
        // OIDC relaxes the PUBLIC bind guard, but NOT the internal one.
        assert!(enforce_loopback_for_unauthenticated(AuthProviderKind::Oidc, "0.0.0.0").is_ok());
        assert!(enforce_internal_listener_safe("0.0.0.0", false).is_err());
    }

    #[test]
    fn loopback_recognition() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("127.0.0.42"));
        assert!(is_loopback("::1"));
        assert!(is_loopback("localhost"));
        assert!(is_loopback("LOCALHOST"));

        assert!(!is_loopback("0.0.0.0"));
        assert!(!is_loopback("::"));
        assert!(!is_loopback("10.0.0.1"));
        assert!(!is_loopback("192.168.1.1"));
        assert!(!is_loopback("example.com"));
        assert!(!is_loopback(""));
    }

    #[test]
    fn oidc_mode_never_blocks_bind() {
        assert!(enforce_loopback_for_unauthenticated(AuthProviderKind::Oidc, "0.0.0.0").is_ok());
        assert!(enforce_loopback_for_unauthenticated(AuthProviderKind::Oidc, "10.0.0.5").is_ok());
    }

    #[test]
    fn local_mode_requires_loopback() {
        assert!(enforce_loopback_for_unauthenticated(AuthProviderKind::Local, "127.0.0.1").is_ok());
        let err = enforce_loopback_for_unauthenticated(AuthProviderKind::Local, "0.0.0.0")
            .expect_err("0.0.0.0 must be rejected in local mode");
        assert!(err.contains("AUTH_PROVIDER=local"));
        assert!(err.contains("0.0.0.0"));
    }

    #[test]
    fn trust_proxy_mode_requires_loopback() {
        assert!(enforce_loopback_for_unauthenticated(AuthProviderKind::TrustProxy, "::1").is_ok());
        let err = enforce_loopback_for_unauthenticated(AuthProviderKind::TrustProxy, "192.168.1.1")
            .expect_err("192.168.x.x must be rejected in trust_proxy mode");
        assert!(err.contains("AUTH_PROVIDER=trust_proxy"));
    }
}
