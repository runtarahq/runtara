pub mod auth;
pub mod cleanup;
pub mod client;
pub mod compilation_progress;
pub mod compilation_queue;
pub mod events;
pub mod stream;

use redis::RedisError;
use redis::aio::ConnectionManager;
use tokio::sync::OnceCell;

/// Process-wide shared Redis connection manager.
///
/// Built lazily on first use (or eagerly at server startup via
/// [`init_shared_manager`]) and shared across every subsystem that talks
/// to Valkey. The manager itself wraps an `Arc`, so cloning is cheap and
/// every clone reuses the same multiplexed connection — no new TCP per
/// request.
///
/// The URL is captured at first initialization. The server runs against a
/// single Valkey instance whose URL is fixed for the process lifetime, so
/// caching a single manager (rather than keying by URL) is intentional.
///
/// # Non-blocking commands only
///
/// `ConnectionManager` is a *single multiplexed connection*. A blocking
/// command issued through this manager parks that connection and
/// head-of-line blocks every other caller until it returns. Callers that
/// issue any of the following commands MUST use
/// [`dedicated_manager_for_blocking_consumer`] instead:
///
/// - `BLPOP`, `BRPOP`, `BRPOPLPUSH`, `BLMOVE`
/// - `BZPOPMIN`, `BZPOPMAX`
/// - `XREAD ... BLOCK`, `XREADGROUP ... BLOCK`
/// - `SUBSCRIBE`, `PSUBSCRIBE`
/// - `WAIT`
/// - any long-running Lua / `EVAL` script
///
/// Production incident on 2026-05-13 (commit `8c43211`): the compilation
/// worker's `BLPOP` was on this shared manager and stalled proxy
/// rate-limit checks to 3–6 s. Do not re-introduce that pattern.
static SHARED_MANAGER: OnceCell<ConnectionManager> = OnceCell::const_new();

/// Return the shared connection manager, building it on first call.
///
/// **Use only for non-blocking commands.** See the warning on
/// [`SHARED_MANAGER`]. For blocking consumers call
/// [`dedicated_manager_for_blocking_consumer`].
///
/// Subsequent calls are O(1) clones. Returns an error only if Redis is
/// unreachable on the very first call (subsequent reconnects are handled
/// transparently by `ConnectionManager`).
pub async fn get_or_create_manager(redis_url: &str) -> Result<ConnectionManager, RedisError> {
    SHARED_MANAGER
        .get_or_try_init(|| async {
            let client = open_client(redis_url)?;
            ConnectionManager::new(client).await
        })
        .await
        .cloned()
}

/// Eagerly initialize the shared manager at startup. Safe to call multiple
/// times; only the first call performs the connection.
pub async fn init_shared_manager(redis_url: &str) -> Result<ConnectionManager, RedisError> {
    get_or_create_manager(redis_url).await
}

/// Build a fresh, isolated `ConnectionManager` for a consumer that issues
/// blocking Redis commands (`BLPOP`, `XREADGROUP ... BLOCK`, `SUBSCRIBE`,
/// …).
///
/// Every call returns an independent manager backed by its own connection.
/// The point is isolation — do NOT share the returned handle with the
/// shared manager's callers, or you negate the benefit. `consumer_name`
/// is used for log/trace context only.
///
/// A grep for this function name enumerates every blocking Redis consumer
/// in the codebase, which is the second purpose of routing through it.
pub async fn dedicated_manager_for_blocking_consumer(
    redis_url: &str,
    consumer_name: &str,
) -> Result<ConnectionManager, RedisError> {
    let client = open_client(redis_url)?;
    let manager = ConnectionManager::new(client).await?;
    tracing::debug!(
        consumer = consumer_name,
        "Opened dedicated Redis ConnectionManager for blocking consumer"
    );
    Ok(manager)
}

/// Open a `redis::Client` for `redis_url`, honoring Valkey TLS settings.
///
/// Every client the server creates must go through here (a grep for this
/// function enumerates them). For a `rediss://` URL with `VALKEY_TLS_CA_CERT`
/// set, the referenced PEM certificate becomes the trusted root for server
/// verification — this is how a self-signed Valkey certificate is trusted
/// without disabling verification. Insecure-mode URLs (`…/#insecure`) and
/// plaintext URLs open directly; a plain `rediss://` URL without a CA
/// override verifies against the platform trust store.
pub fn open_client(redis_url: &str) -> Result<redis::Client, RedisError> {
    let ca_path = std::env::var("VALKEY_TLS_CA_CERT").ok();
    open_client_with_ca(redis_url, ca_path.as_deref())
}

/// Testable core of [`open_client`]: the CA path is a parameter instead of an
/// ambient env read, so unit tests are hermetic.
fn open_client_with_ca(
    redis_url: &str,
    ca_cert_path: Option<&str>,
) -> Result<redis::Client, RedisError> {
    let ca_cert_path = ca_cert_path.map(str::trim).filter(|p| !p.is_empty());
    if redis_url.starts_with("rediss://")
        && !redis_url.ends_with("#insecure")
        && let Some(ca_path) = ca_cert_path
    {
        let root_cert = std::fs::read(ca_path).map_err(|e| {
            RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "failed to read VALKEY_TLS_CA_CERT",
                format!("{}: {}", ca_path, e),
            ))
        })?;
        // rustls_pemfile silently yields zero certificates for non-PEM
        // bytes (a DER cert, a private-key PEM, an empty file), leaving an
        // empty trust store whose only symptom is a generic UnknownIssuer
        // on every connect. Reject those here, where the error can still
        // name VALKEY_TLS_CA_CERT.
        if !String::from_utf8_lossy(&root_cert).contains("-----BEGIN CERTIFICATE-----") {
            return Err(RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "VALKEY_TLS_CA_CERT contains no PEM certificate",
                format!(
                    "{}: expected at least one '-----BEGIN CERTIFICATE-----' block \
                     (is the file DER-encoded or a private key?)",
                    ca_path
                ),
            )));
        }
        return redis::Client::build_with_tls(
            redis_url,
            redis::TlsCertificates {
                client_tls: None,
                root_cert: Some(root_cert),
            },
        );
    }
    redis::Client::open(redis_url)
}

/// Redact the userinfo (user:password) of a `redis://` / `rediss://` URL for
/// safe logging: everything between `scheme://` and the `@` before the host
/// becomes `***`. URLs without credentials pass through unchanged.
pub fn redact_credentials(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme_end), Some(at)) if at > scheme_end => {
            format!("{}***{}", &url[..scheme_end + 3], &url[at..])
        }
        _ => url.to_string(),
    }
}

/// Valkey configuration loaded from environment variables
#[derive(Clone)]
pub struct ValkeyConfig {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<String>,
    /// Stream name for raw event capture (legacy)
    pub stream_name: String,
    /// Consumer group for raw event capture (legacy)
    pub consumer_group: String,
    /// Stream prefix for trigger events (default: "runtara:triggers")
    pub trigger_stream_prefix: String,
    /// Consumer group for trigger workers (default: "runtara-trigger-workers")
    pub trigger_consumer_group: String,
    /// Approximate max length for trigger streams. XACK only clears the PEL, so
    /// without a cap every published event accumulates forever and slowly OOMs
    /// Valkey. Publishes use `XADD ... MAXLEN ~ N` to bound each stream.
    pub trigger_stream_maxlen: usize,
    /// Connect with TLS (`rediss://`). Set via `VALKEY_TLS`.
    pub tls: bool,
    /// Skip server-certificate verification (`VALKEY_TLS_INSECURE`). Local
    /// testing only — the connection is encrypted but the peer is not
    /// authenticated. Ignored when a CA certificate is configured.
    pub tls_insecure: bool,
    /// Path to a PEM CA certificate to trust for server verification
    /// (`VALKEY_TLS_CA_CERT`) — e.g. a self-signed Valkey certificate.
    /// Consumed by [`open_client`], not by the URL.
    pub tls_ca_cert: Option<String>,
}

/// Manual `Debug` so a logged config can never leak the password.
impl std::fmt::Debug for ValkeyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeyConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("stream_name", &self.stream_name)
            .field("consumer_group", &self.consumer_group)
            .field("trigger_stream_prefix", &self.trigger_stream_prefix)
            .field("trigger_consumer_group", &self.trigger_consumer_group)
            .field("trigger_stream_maxlen", &self.trigger_stream_maxlen)
            .field("tls", &self.tls)
            .field("tls_insecure", &self.tls_insecure)
            .field("tls_ca_cert", &self.tls_ca_cert)
            .finish()
    }
}

/// Default approximate cap on trigger stream length. Generous so a consumer
/// backlog isn't trimmed under normal operation, while still bounding growth.
pub const DEFAULT_TRIGGER_STREAM_MAXLEN: usize = 100_000;

/// Interpret a boolean-ish env-var value, biased so unrecognized values
/// ENABLE the flag. Use only where "on" is the secure direction (VALKEY_TLS):
/// unset, empty, `0`, `false`, `no`, `off` (case-insensitive) → false; any
/// other non-empty value → true, so a typo like `VALKEY_TLS=ture` fails
/// toward the secure side (TLS on, visible connection error) instead of
/// silently downgrading to plaintext.
fn flag_permissive(value: Option<&str>) -> bool {
    match value {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        None => false,
    }
}

/// Strict truthy parse: only explicit `1`, `true`, `yes`, `on`
/// (case-insensitive) enable the flag. Use where "on" weakens security
/// (VALKEY_TLS_INSECURE) so a typo or templating artifact (`null`, `flase`)
/// can never silently disable certificate verification.
fn flag_strict(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

impl ValkeyConfig {
    /// Load Valkey configuration from environment variables
    /// Returns None if VALKEY_HOST is not set (Valkey is optional)
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("VALKEY_HOST").ok()?;

        let port = std::env::var("VALKEY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(6379);

        let user = std::env::var("VALKEY_USER").ok();
        let password = std::env::var("VALKEY_PASSWORD").ok();

        let stream_name =
            std::env::var("VALKEY_STREAM_NAME").unwrap_or_else(|_| "runtara-events".to_string());

        let consumer_group = std::env::var("VALKEY_CONSUMER_GROUP")
            .unwrap_or_else(|_| "runtara-workers".to_string());

        let trigger_stream_prefix = std::env::var("VALKEY_TRIGGER_STREAM_PREFIX")
            .ok()
            .filter(|p| !p.is_empty())
            .and_then(|p| {
                // The prefix is interpolated into a Redis SCAN MATCH glob pattern
                // by the cleanup task (`{prefix}:*`), so it must not contain glob
                // metacharacters — otherwise a misconfigured prefix could make
                // cleanup match (and trim) keys belonging to an unrelated stream
                // family sharing this Valkey instance.
                if p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
                {
                    Some(p)
                } else {
                    tracing::warn!(
                        configured_prefix = %p,
                        "VALKEY_TRIGGER_STREAM_PREFIX contains characters unsafe for a Redis \
                         glob pattern (only alphanumeric, ':', '_', '-' allowed); ignoring and \
                         using the default"
                    );
                    None
                }
            })
            .unwrap_or_else(|| "runtara:triggers".to_string());

        let trigger_consumer_group = std::env::var("VALKEY_TRIGGER_CONSUMER_GROUP")
            .unwrap_or_else(|_| "runtara-trigger-workers".to_string());

        let trigger_stream_maxlen = std::env::var("VALKEY_TRIGGER_STREAM_MAXLEN")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_TRIGGER_STREAM_MAXLEN);

        let tls = flag_permissive(std::env::var("VALKEY_TLS").ok().as_deref());
        let tls_insecure_raw = std::env::var("VALKEY_TLS_INSECURE").ok();
        let tls_insecure = flag_strict(tls_insecure_raw.as_deref());
        if let Some(raw) = &tls_insecure_raw
            && !tls_insecure
            && !matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        {
            tracing::warn!(
                value = %raw,
                "Unrecognized VALKEY_TLS_INSECURE value — treating as FALSE \
                 (certificate verification stays on); use 1/true/yes/on to disable it"
            );
        }
        let tls_ca_cert = std::env::var("VALKEY_TLS_CA_CERT")
            .ok()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());

        if user.is_some() && password.is_none() {
            tracing::warn!(
                "VALKEY_USER is set but VALKEY_PASSWORD is not — the username is ignored \
                 and the connection will NOT authenticate; set VALKEY_PASSWORD as well"
            );
        }
        if !tls && (tls_insecure || tls_ca_cert.is_some()) {
            tracing::warn!(
                "VALKEY_TLS_INSECURE / VALKEY_TLS_CA_CERT are set but VALKEY_TLS is not — \
                 connecting in plaintext; set VALKEY_TLS=1 to enable TLS"
            );
        }
        if tls && tls_insecure && tls_ca_cert.is_some() {
            tracing::warn!(
                "Both VALKEY_TLS_CA_CERT and VALKEY_TLS_INSECURE are set — the CA certificate \
                 takes precedence and the server certificate WILL be verified"
            );
        }

        Some(ValkeyConfig {
            host,
            port,
            user,
            password,
            stream_name,
            consumer_group,
            trigger_stream_prefix,
            trigger_consumer_group,
            trigger_stream_maxlen,
            tls,
            tls_insecure,
            tls_ca_cert,
        })
    }

    /// Build Redis connection URL from config
    /// Format: {redis|rediss}://[user:password@]host:port[/#insecure]
    ///
    /// Credentials are percent-encoded (redis-rs percent-decodes them when
    /// parsing), so passwords containing `@ / : # %` survive the round trip.
    /// IPv6 hosts are bracketed per RFC 3986.
    pub fn connection_url(&self) -> String {
        let scheme = if self.tls { "rediss" } else { "redis" };
        // `#insecure` is only honored for rediss:// URLs; a configured CA cert
        // wins over insecure mode so verification is never silently disabled
        // when a trust root is available.
        let suffix = if self.tls && self.tls_insecure && self.tls_ca_cert.is_none() {
            "/#insecure"
        } else {
            ""
        };
        let host: std::borrow::Cow<'_, str> =
            if self.host.contains(':') && !self.host.starts_with('[') {
                format!("[{}]", self.host).into()
            } else {
                self.host.as_str().into()
            };
        match (&self.user, &self.password) {
            (Some(user), Some(password)) => format!(
                "{}://{}:{}@{}:{}{}",
                scheme,
                urlencoding::encode(user),
                urlencoding::encode(password),
                host,
                self.port,
                suffix
            ),
            (None, Some(password)) => format!(
                "{}://:{}@{}:{}{}",
                scheme,
                urlencoding::encode(password),
                host,
                self.port,
                suffix
            ),
            _ => format!("{}://{}:{}{}", scheme, host, self.port, suffix),
        }
    }

    /// Get the trigger stream key for a specific tenant
    /// Format: {trigger_stream_prefix}:{tenant_id}
    pub fn trigger_stream_key(&self, tenant_id: &str) -> String {
        format!("{}:{}", self.trigger_stream_prefix, tenant_id)
    }
}

/// Build Redis connection URL from environment variables
/// Returns None if VALKEY_HOST is not set
pub fn build_redis_url() -> Option<String> {
    ValkeyConfig::from_env().map(|config| config.connection_url())
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::{ConnectionAddr, IntoConnectionInfo};

    fn base_config() -> ValkeyConfig {
        ValkeyConfig {
            host: "localhost".to_string(),
            port: 6379,
            user: None,
            password: None,
            stream_name: "runtara-events".to_string(),
            consumer_group: "runtara-workers".to_string(),
            trigger_stream_prefix: "runtara:triggers".to_string(),
            trigger_consumer_group: "runtara-trigger-workers".to_string(),
            trigger_stream_maxlen: DEFAULT_TRIGGER_STREAM_MAXLEN,
            tls: false,
            tls_insecure: false,
            tls_ca_cert: None,
        }
    }

    #[test]
    fn plain_url_unchanged() {
        assert_eq!(base_config().connection_url(), "redis://localhost:6379");
    }

    #[test]
    fn tls_url_uses_rediss_scheme() {
        let cfg = ValkeyConfig {
            tls: true,
            ..base_config()
        };
        let url = cfg.connection_url();
        assert_eq!(url, "rediss://localhost:6379");
        let info = url.as_str().into_connection_info().expect("parse");
        assert!(matches!(
            info.addr,
            ConnectionAddr::TcpTls {
                insecure: false,
                ..
            }
        ));
    }

    #[test]
    fn tls_insecure_appends_fragment() {
        let cfg = ValkeyConfig {
            tls: true,
            tls_insecure: true,
            ..base_config()
        };
        let url = cfg.connection_url();
        assert_eq!(url, "rediss://localhost:6379/#insecure");
        let info = url.as_str().into_connection_info().expect("parse");
        assert!(matches!(
            info.addr,
            ConnectionAddr::TcpTls { insecure: true, .. }
        ));
    }

    #[test]
    fn ca_cert_wins_over_insecure() {
        let cfg = ValkeyConfig {
            tls: true,
            tls_insecure: true,
            tls_ca_cert: Some("/tmp/ca.pem".to_string()),
            ..base_config()
        };
        assert_eq!(cfg.connection_url(), "rediss://localhost:6379");
    }

    #[test]
    fn insecure_without_tls_stays_plaintext() {
        let cfg = ValkeyConfig {
            tls_insecure: true,
            ..base_config()
        };
        assert_eq!(cfg.connection_url(), "redis://localhost:6379");
    }

    #[test]
    fn credentials_survive_percent_encoding_round_trip() {
        let cfg = ValkeyConfig {
            user: Some("app user".to_string()),
            password: Some("p@ss/w:rd#1%?".to_string()),
            tls: true,
            ..base_config()
        };
        let info = cfg
            .connection_url()
            .as_str()
            .into_connection_info()
            .expect("parse URL with special-character credentials");
        assert_eq!(info.redis.username.as_deref(), Some("app user"));
        assert_eq!(info.redis.password.as_deref(), Some("p@ss/w:rd#1%?"));
    }

    #[test]
    fn password_only_percent_encoded() {
        let cfg = ValkeyConfig {
            password: Some("s3cret@!".to_string()),
            ..base_config()
        };
        let info = cfg
            .connection_url()
            .as_str()
            .into_connection_info()
            .expect("parse");
        assert_eq!(info.redis.username, None);
        assert_eq!(info.redis.password.as_deref(), Some("s3cret@!"));
    }

    #[test]
    fn ipv6_host_is_bracketed() {
        let cfg = ValkeyConfig {
            host: "::1".to_string(),
            tls: true,
            ..base_config()
        };
        let url = cfg.connection_url();
        assert_eq!(url, "rediss://[::1]:6379");
        url.as_str().into_connection_info().expect("parse IPv6 URL");
        // Already-bracketed hosts must not be double-bracketed.
        let cfg = ValkeyConfig {
            host: "[::1]".to_string(),
            ..base_config()
        };
        assert_eq!(cfg.connection_url(), "redis://[::1]:6379");
    }

    #[test]
    fn permissive_flag_fails_toward_enabled() {
        for falsy in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("No"),
            Some("OFF"),
        ] {
            assert!(!flag_permissive(falsy), "{falsy:?} should be false");
        }
        for truthy in [
            Some("1"),
            Some("true"),
            Some("TRUE"),
            Some("yes"),
            Some("ture"),
        ] {
            assert!(
                flag_permissive(truthy),
                "{truthy:?} should be true (fail-secure for VALKEY_TLS)"
            );
        }
    }

    #[test]
    fn strict_flag_fails_toward_disabled() {
        for truthy in [Some("1"), Some("true"), Some("YES"), Some("On")] {
            assert!(flag_strict(truthy), "{truthy:?} should be true");
        }
        // Typos and templating artifacts must NOT disable verification.
        for other in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("ture"),
            Some("flase"),
            Some("null"),
            Some("None"),
            Some("disabled"),
        ] {
            assert!(
                !flag_strict(other),
                "{other:?} must be false (fail-secure for VALKEY_TLS_INSECURE)"
            );
        }
    }

    #[test]
    fn open_client_accepts_all_url_shapes() {
        // No live connection is made at client-open time; this validates URL
        // parsing and TLS feature wiring for every shape connection_url emits.
        // Uses the parameterized core (CA = None) so an ambient
        // VALKEY_TLS_CA_CERT in the test environment cannot change behavior.
        for cfg in [
            base_config(),
            ValkeyConfig {
                tls: true,
                ..base_config()
            },
            ValkeyConfig {
                tls: true,
                tls_insecure: true,
                ..base_config()
            },
            ValkeyConfig {
                tls: true,
                user: Some("u".into()),
                password: Some("p@ss".into()),
                ..base_config()
            },
        ] {
            open_client_with_ca(&cfg.connection_url(), None).expect("open client");
        }
    }

    /// A valid (self-signed, long-lived) certificate for exercising the CA
    /// branch without touching the network or the process environment.
    const TEST_ANCHOR_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBizCCATGgAwIBAgIUHpMjXgUQZpui6YkImEVlPC6o0CkwCgYIKoZIzj0EAwIw
GzEZMBcGA1UEAwwQdW5pdC10ZXN0LWFuY2hvcjAeFw0yNjA3MDkwNTE3MTBaFw0z
NjA3MDYwNTE3MTBaMBsxGTAXBgNVBAMMEHVuaXQtdGVzdC1hbmNob3IwWTATBgcq
hkjOPQIBBggqhkjOPQMBBwNCAATHDiMm00z/3kBd0HltG5kHtw/j1bnq1AJnbvB3
3EAeJjEZrzWIIbIr7EWIMVLJsVPWDO8kSVPxA9+g+TKL7Igso1MwUTAdBgNVHQ4E
FgQUpiu6gx1sWiAOrrXkEzEox4+9AW4wHwYDVR0jBBgwFoAUpiu6gx1sWiAOrrXk
EzEox4+9AW4wDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiEA3PoP
LBMr/HB5f5fUU9CFXwG3p0Qf+Al7nsp+omsER/YCIDfE5Uk0BDzSeTT/e4TWBXgG
TyeyC1e+pgLTZVcvk+Bw
-----END CERTIFICATE-----
";

    fn temp_pem(name: &str, contents: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("valkey-tls-unit-{}-{}", std::process::id(), name));
        std::fs::write(&path, contents).expect("write temp pem");
        path
    }

    #[test]
    fn ca_branch_builds_client_from_valid_pem() {
        let path = temp_pem("valid.pem", TEST_ANCHOR_PEM);
        open_client_with_ca("rediss://localhost:6390", path.to_str())
            .expect("valid CA PEM must build a client");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ca_branch_rejects_missing_file_naming_the_env_var() {
        let err = open_client_with_ca("rediss://localhost:6390", Some("/nonexistent/ca.pem"))
            .expect_err("missing CA file must error at client build");
        let msg = err.to_string();
        assert!(
            msg.contains("VALKEY_TLS_CA_CERT"),
            "error must name the env var, got: {msg}"
        );
    }

    #[test]
    fn ca_branch_rejects_non_certificate_pem() {
        // A private key (or DER/empty file) parses to ZERO trust anchors in
        // rustls-pemfile without error — must be rejected up front instead of
        // failing every connect with an unattributable UnknownIssuer.
        let path = temp_pem(
            "key.pem",
            "-----BEGIN PRIVATE KEY-----\nboguskeybytes\n-----END PRIVATE KEY-----\n",
        );
        let err = open_client_with_ca("rediss://localhost:6390", path.to_str())
            .expect_err("non-certificate PEM must error at client build");
        let msg = err.to_string();
        assert!(
            msg.contains("no PEM certificate"),
            "error must explain the problem, got: {msg}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn redact_credentials_masks_userinfo_only() {
        assert_eq!(
            redact_credentials("redis://user:p%40ss@host:6379"),
            "redis://***@host:6379"
        );
        assert_eq!(
            redact_credentials("rediss://:secret@host:6390/#insecure"),
            "rediss://***@host:6390/#insecure"
        );
        // No credentials → unchanged.
        assert_eq!(redact_credentials("redis://host:6379"), "redis://host:6379");
        // Legacy un-encoded password containing '@': rfind keeps the host.
        assert_eq!(
            redact_credentials("redis://:p@ss@host:6379"),
            "redis://***@host:6379"
        );
    }

    #[test]
    fn valkey_config_debug_hides_password() {
        let cfg = ValkeyConfig {
            user: Some("app".to_string()),
            password: Some("hunter2".to_string()),
            ..base_config()
        };
        let debug_str = format!("{cfg:?}");
        assert!(
            !debug_str.contains("hunter2"),
            "password leaked: {debug_str}"
        );
        assert!(debug_str.contains("***"));
        assert!(debug_str.contains("localhost"));
    }

    #[test]
    fn ca_ignored_for_insecure_and_plaintext_urls() {
        // A nonexistent CA path proves the branch is skipped: if either URL
        // shape consulted the CA, client build would fail on the read.
        open_client_with_ca(
            "rediss://localhost:6390/#insecure",
            Some("/nonexistent/ca.pem"),
        )
        .expect("insecure URL must not consult the CA");
        open_client_with_ca("redis://localhost:6390", Some("/nonexistent/ca.pem"))
            .expect("plaintext URL must not consult the CA");
    }
}
