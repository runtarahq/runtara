// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration loading from environment variables.

use std::net::SocketAddr;
use std::time::Duration;

use crate::runtime::DEFAULT_SHUTDOWN_GRACE;

/// Cap the standalone binary applies when `RUNTARA_MAX_CONCURRENT_INSTANCES`
/// is unset. A host that embeds the runtime does not inherit this: see
/// [`RuntimeOverrides`].
const DEFAULT_MAX_CONCURRENT_INSTANCES: u32 = 32;

/// Runtara Core configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL or SQLite connection URL
    pub database_url: String,
    /// HTTP server address for instance communication
    pub http_addr: SocketAddr,
    /// Maximum instances in `running` at once; `0` disables the cap
    pub max_concurrent_instances: u32,
    /// How long shutdown waits for in-flight requests before aborting them
    pub shutdown_grace_ms: u64,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Required:
    /// - `RUNTARA_DATABASE_URL`: PostgreSQL or SQLite connection string
    ///
    /// Optional (with defaults):
    /// - `RUNTARA_HTTP_PORT`: HTTP server port (default: 8001)
    /// - `RUNTARA_MAX_CONCURRENT_INSTANCES`: Max concurrent instances (default: 32)
    /// - `RUNTARA_CORE_SHUTDOWN_GRACE_MS`: How long shutdown waits for in-flight
    ///   instance-protocol requests to finish before it stops waiting (default:
    ///   [`DEFAULT_SHUTDOWN_GRACE`]). Distinct from runtara-server's
    ///   `RUNTARA_SHUTDOWN_GRACE_MS`, which bounds a different phase — the two
    ///   share a process in the embedded server, and their waits are additive.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = std::env::var("RUNTARA_DATABASE_URL")
            .map_err(|_| ConfigError::Missing("RUNTARA_DATABASE_URL"))?;

        let http_port: u16 = std::env::var("RUNTARA_HTTP_PORT")
            .unwrap_or_else(|_| "8001".to_string())
            .parse()
            .map_err(|_| {
                ConfigError::Invalid("RUNTARA_HTTP_PORT", "must be a valid port number")
            })?;

        let max_concurrent_instances =
            max_concurrent_instances_from_env()?.unwrap_or(DEFAULT_MAX_CONCURRENT_INSTANCES);

        let shutdown_grace_ms = shutdown_grace_from_env()?
            // Read the runtime's own constant rather than repeating the number,
            // so the documented default cannot drift from the applied one.
            .unwrap_or(DEFAULT_SHUTDOWN_GRACE)
            .as_millis() as u64;

        Ok(Self {
            database_url,
            http_addr: SocketAddr::from(([0, 0, 0, 0], http_port)),
            max_concurrent_instances,
            shutdown_grace_ms,
        })
    }
}

/// The [`CoreRuntime`](crate::runtime::CoreRuntime) knobs a host that *embeds*
/// the runtime can take from the environment, each `None` when its variable is
/// unset.
///
/// [`Config`] is for a process that *is* runtara-core: it owns the database URL
/// and the listen port, and it substitutes its own default for anything unset.
/// An embedding host supplies persistence and bind address itself and wants
/// only the rest — and must not have a concurrency cap it never asked for
/// switched on underneath it by an upgrade. Optional fields are what separate
/// "unset" from "set to the same value as the default", so
/// [`apply_overrides`](crate::runtime::CoreRuntimeBuilder::apply_overrides) can
/// leave the builder's own default in place.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeOverrides {
    /// `RUNTARA_MAX_CONCURRENT_INSTANCES`, when set.
    pub max_concurrent_instances: Option<u32>,
    /// `RUNTARA_CORE_SHUTDOWN_GRACE_MS`, when set.
    pub shutdown_grace: Option<Duration>,
}

impl RuntimeOverrides {
    /// Read both variables from the environment.
    ///
    /// An unset variable is not an error — it yields `None`. A malformed one
    /// is, so a typo in a deployment's configuration fails startup instead of
    /// being silently ignored.
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            max_concurrent_instances: max_concurrent_instances_from_env()?,
            shutdown_grace: shutdown_grace_from_env()?,
        })
    }
}

/// Parse `RUNTARA_MAX_CONCURRENT_INSTANCES`, returning `None` when it is unset
/// so each caller can apply its own default.
fn max_concurrent_instances_from_env() -> Result<Option<u32>, ConfigError> {
    match std::env::var("RUNTARA_MAX_CONCURRENT_INSTANCES") {
        Ok(raw) => raw.parse::<u32>().map(Some).map_err(|_| {
            ConfigError::Invalid(
                "RUNTARA_MAX_CONCURRENT_INSTANCES",
                "must be a positive integer",
            )
        }),
        Err(_) => Ok(None),
    }
}

/// Parse `RUNTARA_CORE_SHUTDOWN_GRACE_MS`, returning `None` when it is unset.
fn shutdown_grace_from_env() -> Result<Option<Duration>, ConfigError> {
    match std::env::var("RUNTARA_CORE_SHUTDOWN_GRACE_MS") {
        Ok(raw) => raw
            .parse::<u64>()
            .map(|ms| Some(Duration::from_millis(ms)))
            .map_err(|_| {
                ConfigError::Invalid(
                    "RUNTARA_CORE_SHUTDOWN_GRACE_MS",
                    "must be a non-negative integer number of milliseconds",
                )
            }),
        Err(_) => Ok(None),
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable is missing.
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),

    /// An environment variable has an invalid value.
    #[error("invalid value for {0}: {1}")]
    Invalid(&'static str, &'static str),
}

/// Resolve a boolean "enabled" env var with a "default-on, opt-out only" rule.
///
/// Returns `true` unless the env var is set to a recognised false-like value
/// (case-insensitive, trimmed): `"false"`, `"0"`, `"no"`, `"off"`, or
/// `"disabled"`. **Any other value — including unset, malformed input, typos,
/// or truthy spellings like `"yes"`/`"on"`/`"True"` — leaves the feature
/// enabled.** This is the inverse of the naive `v == "true" || v == "1"`
/// parse: it cannot silently disable a feature because of a misconfiguration.
///
/// Used by all four cleanup workers (`*_CLEANUP_ENABLED`); pull it in via
/// `use runtara_core::config::parse_enabled_env;`.
pub fn parse_enabled_env(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off" | "disabled"
        ),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Helper to set env vars for a test and restore them after
    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new() -> Self {
            Self { vars: Vec::new() }
        }

        fn set(&mut self, key: &str, value: &str) {
            let old = env::var(key).ok();
            self.vars.push((key.to_string(), old));
            // SAFETY: Tests are serialized via ENV_MUTEX, so no concurrent access
            unsafe { env::set_var(key, value) };
        }

        fn remove(&mut self, key: &str) {
            let old = env::var(key).ok();
            self.vars.push((key.to_string(), old));
            // SAFETY: Tests are serialized via ENV_MUTEX, so no concurrent access
            unsafe { env::remove_var(key) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.vars.drain(..).rev() {
                // SAFETY: Tests are serialized via ENV_MUTEX, so no concurrent access
                unsafe {
                    match value {
                        Some(v) => env::set_var(&key, v),
                        None => env::remove_var(&key),
                    }
                }
            }
        }
    }

    #[test]
    fn test_config_from_env_with_defaults() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.remove("RUNTARA_HTTP_PORT");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "postgres://localhost/test");
        assert_eq!(config.http_addr.port(), 8001);
        assert_eq!(config.max_concurrent_instances, 32);
    }

    #[test]
    fn test_config_from_env_with_custom_port() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "sqlite:test.db");
        guard.set("RUNTARA_HTTP_PORT", "9999");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "sqlite:test.db");
        assert_eq!(config.http_addr.port(), 9999);
        assert_eq!(config.max_concurrent_instances, 32);
    }

    #[test]
    fn test_config_from_env_with_custom_max_instances() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.remove("RUNTARA_HTTP_PORT");
        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "100");

        let config = Config::from_env().unwrap();

        assert_eq!(config.max_concurrent_instances, 100);
    }

    #[test]
    fn test_config_from_env_all_custom() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://user:pass@db:5432/prod");
        guard.set("RUNTARA_HTTP_PORT", "8080");
        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "256");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "postgres://user:pass@db:5432/prod");
        assert_eq!(config.http_addr.port(), 8080);
        assert_eq!(config.max_concurrent_instances, 256);
    }

    #[test]
    fn test_config_missing_database_url() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.remove("RUNTARA_DATABASE_URL");

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::Missing("RUNTARA_DATABASE_URL")));
        assert!(err.to_string().contains("RUNTARA_DATABASE_URL"));
    }

    #[test]
    fn test_config_invalid_http_port() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_HTTP_PORT", "not_a_number");

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::Invalid("RUNTARA_HTTP_PORT", _)));
    }

    #[test]
    fn test_config_invalid_http_port_out_of_range() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_HTTP_PORT", "99999"); // > 65535

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::Invalid("RUNTARA_HTTP_PORT", _)));
    }

    #[test]
    fn test_config_invalid_max_concurrent_instances() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "abc");

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid("RUNTARA_MAX_CONCURRENT_INSTANCES", _)
        ));
    }

    #[test]
    fn test_config_negative_max_concurrent_instances() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "-5");

        let result = Config::from_env();
        assert!(result.is_err());
    }

    #[test]
    fn test_runtime_overrides_unset_are_none() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");
        guard.remove("RUNTARA_CORE_SHUTDOWN_GRACE_MS");

        // `None`, not the standalone binary's defaults: an embedding host must
        // keep its own behaviour when a deployment sets nothing.
        assert_eq!(
            RuntimeOverrides::from_env().unwrap(),
            RuntimeOverrides::default()
        );
    }

    #[test]
    fn test_runtime_overrides_read_both_variables() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "7");
        guard.set("RUNTARA_CORE_SHUTDOWN_GRACE_MS", "1500");

        let overrides = RuntimeOverrides::from_env().unwrap();

        assert_eq!(overrides.max_concurrent_instances, Some(7));
        assert_eq!(overrides.shutdown_grace, Some(Duration::from_millis(1500)));
    }

    #[test]
    fn test_runtime_overrides_zero_cap_is_explicit() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "0");
        guard.remove("RUNTARA_CORE_SHUTDOWN_GRACE_MS");

        // `Some(0)`, distinct from unset — disabling the cap is a choice a
        // deployment can make, not the absence of one.
        assert_eq!(
            RuntimeOverrides::from_env()
                .unwrap()
                .max_concurrent_instances,
            Some(0)
        );
    }

    #[test]
    fn test_runtime_overrides_invalid_values_are_errors() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "lots");
        guard.remove("RUNTARA_CORE_SHUTDOWN_GRACE_MS");
        assert!(matches!(
            RuntimeOverrides::from_env().unwrap_err(),
            ConfigError::Invalid("RUNTARA_MAX_CONCURRENT_INSTANCES", _)
        ));

        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");
        guard.set("RUNTARA_CORE_SHUTDOWN_GRACE_MS", "30s");
        assert!(matches!(
            RuntimeOverrides::from_env().unwrap_err(),
            ConfigError::Invalid("RUNTARA_CORE_SHUTDOWN_GRACE_MS", _)
        ));
    }

    #[test]
    fn test_config_defaults_where_overrides_are_none() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");
        guard.remove("RUNTARA_CORE_SHUTDOWN_GRACE_MS");

        let config = Config::from_env().unwrap();

        assert!(
            RuntimeOverrides::from_env()
                .unwrap()
                .shutdown_grace
                .is_none()
        );
        assert_eq!(
            config.max_concurrent_instances,
            DEFAULT_MAX_CONCURRENT_INSTANCES
        );
        assert_eq!(
            config.shutdown_grace_ms,
            DEFAULT_SHUTDOWN_GRACE.as_millis() as u64
        );
    }

    #[test]
    fn test_config_error_display() {
        let missing = ConfigError::Missing("MY_VAR");
        assert_eq!(
            missing.to_string(),
            "missing required environment variable: MY_VAR"
        );

        let invalid = ConfigError::Invalid("MY_VAR", "must be a number");
        assert_eq!(
            invalid.to_string(),
            "invalid value for MY_VAR: must be a number"
        );
    }

    #[test]
    fn test_config_debug() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.remove("RUNTARA_HTTP_PORT");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("database_url"));
        assert!(debug_str.contains("http_addr"));
    }

    #[test]
    fn test_config_clone() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.remove("RUNTARA_HTTP_PORT");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();
        let cloned = config.clone();

        assert_eq!(config.database_url, cloned.database_url);
        assert_eq!(config.http_addr, cloned.http_addr);
        assert_eq!(
            config.max_concurrent_instances,
            cloned.max_concurrent_instances
        );
    }

    #[test]
    fn test_parse_enabled_env_default_on() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();
        const VAR: &str = "RUNTARA_TEST_PARSE_ENABLED_HELPER";

        // Unset → enabled
        guard.remove(VAR);
        assert!(parse_enabled_env(VAR), "unset must be enabled");

        // Truthy spellings (and typos) leave the feature on
        for v in [
            "true",
            "1",
            "yes",
            "on",
            "True",
            "TRUE",
            "anything-else",
            "",
            "  true  ",
        ] {
            guard.set(VAR, v);
            assert!(
                parse_enabled_env(VAR),
                "{v:?} must NOT silently disable the feature"
            );
        }

        // Only explicit false-like values disable
        for v in [
            "false",
            "0",
            "no",
            "off",
            "disabled",
            "FALSE",
            "Off",
            "  false  ",
        ] {
            guard.set(VAR, v);
            assert!(!parse_enabled_env(VAR), "{v:?} must explicitly disable");
        }
    }
}
