// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Deployment overrides for the server's instance runtime.

use std::time::Duration;

use super::ConfigError;

const MAX_CONCURRENT_INSTANCES: &str = "RUNTARA_MAX_CONCURRENT_INSTANCES";
const SHUTDOWN_GRACE_MS: &str = "RUNTARA_CORE_SHUTDOWN_GRACE_MS";
const RUNTIME_MAX_CONNECTIONS: &str = "RUNTARA_RUNTIME_MAX_CONNECTIONS";
const POOL_ACQUIRE_TIMEOUT_SECS: &str = "RUNTARA_RUNTIME_POOL_ACQUIRE_TIMEOUT_SECS";
const POOL_IDLE_TIMEOUT_SECS: &str = "RUNTARA_RUNTIME_POOL_IDLE_TIMEOUT_SECS";
const POOL_MAX_LIFETIME_SECS: &str = "RUNTARA_RUNTIME_POOL_MAX_LIFETIME_SECS";

/// Optional settings applied by the host to its instance runtime.
///
/// `None` leaves the builder's existing value unchanged. Explicit zero values
/// disable the concurrency cap or make the shutdown grace immediate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeOverrides {
    /// Maximum number of running instances, when configured.
    pub max_concurrent_instances: Option<u32>,
    /// How long the server waits for in-flight instance requests to finish.
    pub shutdown_grace: Option<Duration>,
}

impl RuntimeOverrides {
    /// Load the server's instance-runtime overrides from the process environment.
    ///
    /// Reads `RUNTARA_MAX_CONCURRENT_INSTANCES` and
    /// `RUNTARA_CORE_SHUTDOWN_GRACE_MS`. Unset variables leave builder defaults
    /// unchanged; malformed values fail embedded startup before storage opens.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_raw(
            std::env::var(MAX_CONCURRENT_INSTANCES).ok().as_deref(),
            std::env::var(SHUTDOWN_GRACE_MS).ok().as_deref(),
        )
    }

    fn from_raw(cap: Option<&str>, grace_ms: Option<&str>) -> Result<Self, ConfigError> {
        let max_concurrent_instances = cap
            .map(|raw| {
                raw.parse::<u32>().map_err(|_| {
                    ConfigError::Invalid(MAX_CONCURRENT_INSTANCES, "must be a non-negative integer")
                })
            })
            .transpose()?;
        let shutdown_grace = grace_ms
            .map(|raw| {
                raw.parse::<u64>().map(Duration::from_millis).map_err(|_| {
                    ConfigError::Invalid(
                        SHUTDOWN_GRACE_MS,
                        "must be a non-negative integer number of milliseconds",
                    )
                })
            })
            .transpose()?;
        Ok(Self {
            max_concurrent_instances,
            shutdown_grace,
        })
    }
}

/// Default maximum connections for the runtime pool. Every instance launch and
/// every wake does several round trips on it, so this is a direct cap on how
/// fast instances start and resume.
pub const DEFAULT_RUNTIME_MAX_CONNECTIONS: u32 = 32;
/// Default wait for a free connection, in seconds. Matches sqlx's own default.
pub const DEFAULT_POOL_ACQUIRE_TIMEOUT_SECS: u64 = 30;
/// Default idle recycle, in seconds. Matches sqlx's own default.
pub const DEFAULT_POOL_IDLE_TIMEOUT_SECS: u64 = 600;
/// Default connection lifetime, in seconds. Matches sqlx's own default.
pub const DEFAULT_POOL_MAX_LIFETIME_SECS: u64 = 1800;

/// Connection-pool tuning for the runtime database the embedded runtime owns.
///
/// These map directly onto sqlx's `PgPoolOptions`, and the timeout defaults are
/// sqlx's own values made explicit — an unconfigured deployment keeps the
/// behaviour it already had. A `None` `idle_timeout`/`max_lifetime` disables
/// that timeout, which is what a configured `0` means.
///
/// The knobs apply only where this process owns the pool. Nothing here reaches
/// a pool constructed elsewhere and handed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimePoolConfig {
    /// Maximum connections in the pool.
    pub max_connections: u32,
    /// How long to wait for a free connection before erroring.
    pub acquire_timeout: Duration,
    /// Close a connection after it has been idle this long (`None` = never).
    pub idle_timeout: Option<Duration>,
    /// Close a connection after it has lived this long (`None` = never).
    pub max_lifetime: Option<Duration>,
}

impl Default for RuntimePoolConfig {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_RUNTIME_MAX_CONNECTIONS,
            acquire_timeout: Duration::from_secs(DEFAULT_POOL_ACQUIRE_TIMEOUT_SECS),
            idle_timeout: Some(Duration::from_secs(DEFAULT_POOL_IDLE_TIMEOUT_SECS)),
            max_lifetime: Some(Duration::from_secs(DEFAULT_POOL_MAX_LIFETIME_SECS)),
        }
    }
}

impl RuntimePoolConfig {
    /// Load the runtime pool's tuning from the process environment.
    ///
    /// Reads `RUNTARA_RUNTIME_MAX_CONNECTIONS`,
    /// `RUNTARA_RUNTIME_POOL_ACQUIRE_TIMEOUT_SECS`,
    /// `RUNTARA_RUNTIME_POOL_IDLE_TIMEOUT_SECS` and
    /// `RUNTARA_RUNTIME_POOL_MAX_LIFETIME_SECS`. Unset variables keep the
    /// defaults; malformed values fail embedded startup before storage opens,
    /// rather than reverting to a default the operator never asked for.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_raw(
            std::env::var(RUNTIME_MAX_CONNECTIONS).ok().as_deref(),
            std::env::var(POOL_ACQUIRE_TIMEOUT_SECS).ok().as_deref(),
            std::env::var(POOL_IDLE_TIMEOUT_SECS).ok().as_deref(),
            std::env::var(POOL_MAX_LIFETIME_SECS).ok().as_deref(),
        )
    }

    fn from_raw(
        max_connections: Option<&str>,
        acquire_secs: Option<&str>,
        idle_secs: Option<&str>,
        lifetime_secs: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let max_connections = match max_connections {
            Some(raw) => raw.parse::<u32>().map_err(|_| {
                ConfigError::Invalid(RUNTIME_MAX_CONNECTIONS, "must be a positive integer")
            })?,
            None => DEFAULT_RUNTIME_MAX_CONNECTIONS,
        };
        // A pool of zero can never hand out a connection, so it is a
        // misconfiguration and not a way to spell "use the default".
        if max_connections == 0 {
            return Err(ConfigError::Invalid(
                RUNTIME_MAX_CONNECTIONS,
                "must be a positive integer",
            ));
        }
        Ok(Self {
            max_connections,
            acquire_timeout: Duration::from_secs(parse_secs(
                POOL_ACQUIRE_TIMEOUT_SECS,
                acquire_secs,
                DEFAULT_POOL_ACQUIRE_TIMEOUT_SECS,
            )?),
            idle_timeout: super::secs_to_opt_duration(parse_secs(
                POOL_IDLE_TIMEOUT_SECS,
                idle_secs,
                DEFAULT_POOL_IDLE_TIMEOUT_SECS,
            )?),
            max_lifetime: super::secs_to_opt_duration(parse_secs(
                POOL_MAX_LIFETIME_SECS,
                lifetime_secs,
                DEFAULT_POOL_MAX_LIFETIME_SECS,
            )?),
        })
    }
}

fn parse_secs(name: &'static str, raw: Option<&str>, default: u64) -> Result<u64, ConfigError> {
    match raw {
        Some(raw) => raw.parse::<u64>().map_err(|_| {
            ConfigError::Invalid(name, "must be a non-negative integer number of seconds")
        }),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_process_environment() {
        const CASE: &str = "RUNTARA_TEST_RUNTIME_CONFIG_CASE";
        if let Ok(case) = std::env::var(CASE) {
            let actual = RuntimeOverrides::from_env();
            match case.as_str() {
                "unset" => assert_eq!(actual.unwrap(), RuntimeOverrides::default()),
                "set" => assert_eq!(
                    actual.unwrap(),
                    RuntimeOverrides {
                        max_concurrent_instances: Some(7),
                        shutdown_grace: Some(Duration::from_millis(1500)),
                    }
                ),
                "invalid" => assert!(matches!(
                    actual,
                    Err(ConfigError::Invalid(MAX_CONCURRENT_INSTANCES, _))
                )),
                _ => panic!("unknown test case"),
            }
            return;
        }

        // Each child gets its own environment; no process-global mutation or
        // serialization is needed alongside the server's other tests.
        for case in ["unset", "set", "invalid"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "config::runtime::tests::loads_process_environment",
                    "--nocapture",
                ])
                .env(CASE, case)
                .env_remove(MAX_CONCURRENT_INSTANCES)
                .env_remove(SHUTDOWN_GRACE_MS);
            if case == "set" {
                child
                    .env(MAX_CONCURRENT_INSTANCES, "7")
                    .env(SHUTDOWN_GRACE_MS, "1500");
            } else if case == "invalid" {
                child.env(MAX_CONCURRENT_INSTANCES, "lots");
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "{case}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn unset_overrides_leave_host_defaults_alone() {
        assert_eq!(
            RuntimeOverrides::from_raw(None, None).unwrap(),
            RuntimeOverrides::default()
        );
    }

    #[test]
    fn parses_independent_overrides_and_preserves_explicit_zero() {
        assert_eq!(
            RuntimeOverrides::from_raw(Some("7"), Some("1500")).unwrap(),
            RuntimeOverrides {
                max_concurrent_instances: Some(7),
                shutdown_grace: Some(Duration::from_millis(1500)),
            }
        );
        assert_eq!(
            RuntimeOverrides::from_raw(Some("0"), None).unwrap(),
            RuntimeOverrides {
                max_concurrent_instances: Some(0),
                shutdown_grace: None
            }
        );
        assert_eq!(
            RuntimeOverrides::from_raw(None, Some("0")).unwrap(),
            RuntimeOverrides {
                max_concurrent_instances: None,
                shutdown_grace: Some(Duration::ZERO)
            }
        );
    }

    #[test]
    fn rejects_malformed_negative_and_overflowing_values() {
        for raw in ["", "lots", "-1", "4294967296"] {
            assert!(
                matches!(
                    RuntimeOverrides::from_raw(Some(raw), None),
                    Err(ConfigError::Invalid(MAX_CONCURRENT_INSTANCES, _))
                ),
                "cap {raw:?}"
            );
        }
        for raw in ["", "30s", "-1", "18446744073709551616"] {
            assert!(
                matches!(
                    RuntimeOverrides::from_raw(None, Some(raw)),
                    Err(ConfigError::Invalid(SHUTDOWN_GRACE_MS, _))
                ),
                "grace {raw:?}"
            );
        }
    }

    #[test]
    fn unset_pool_variables_keep_the_sqlx_defaults() {
        assert_eq!(
            RuntimePoolConfig::from_raw(None, None, None, None).unwrap(),
            RuntimePoolConfig::default()
        );
        let defaults = RuntimePoolConfig::default();
        assert_eq!(defaults.max_connections, DEFAULT_RUNTIME_MAX_CONNECTIONS);
        assert_eq!(defaults.acquire_timeout, Duration::from_secs(30));
        assert_eq!(defaults.idle_timeout, Some(Duration::from_secs(600)));
        assert_eq!(defaults.max_lifetime, Some(Duration::from_secs(1800)));
    }

    #[test]
    fn parses_each_pool_knob_independently() {
        assert_eq!(
            RuntimePoolConfig::from_raw(Some("64"), Some("5"), Some("120"), Some("900")).unwrap(),
            RuntimePoolConfig {
                max_connections: 64,
                acquire_timeout: Duration::from_secs(5),
                idle_timeout: Some(Duration::from_secs(120)),
                max_lifetime: Some(Duration::from_secs(900)),
            }
        );
        assert_eq!(
            RuntimePoolConfig::from_raw(Some("8"), None, None, None).unwrap(),
            RuntimePoolConfig {
                max_connections: 8,
                ..RuntimePoolConfig::default()
            }
        );
    }

    #[test]
    fn zero_seconds_disables_idle_timeout_and_max_lifetime() {
        let config = RuntimePoolConfig::from_raw(None, Some("0"), Some("0"), Some("0")).unwrap();
        assert_eq!(config.acquire_timeout, Duration::ZERO);
        assert_eq!(config.idle_timeout, None);
        assert_eq!(config.max_lifetime, None);
    }

    #[test]
    fn rejects_a_pool_that_can_never_hand_out_a_connection() {
        assert!(matches!(
            RuntimePoolConfig::from_raw(Some("0"), None, None, None),
            Err(ConfigError::Invalid(RUNTIME_MAX_CONNECTIONS, _))
        ));
    }

    #[test]
    fn rejects_malformed_pool_values_instead_of_reverting_to_a_default() {
        for raw in ["", "lots", "-1", "4294967296"] {
            assert!(
                matches!(
                    RuntimePoolConfig::from_raw(Some(raw), None, None, None),
                    Err(ConfigError::Invalid(RUNTIME_MAX_CONNECTIONS, _))
                ),
                "max_connections {raw:?}"
            );
        }
        for (position, name) in [
            (1, POOL_ACQUIRE_TIMEOUT_SECS),
            (2, POOL_IDLE_TIMEOUT_SECS),
            (3, POOL_MAX_LIFETIME_SECS),
        ] {
            for raw in ["", "30s", "-1", "18446744073709551616"] {
                let mut args: [Option<&str>; 3] = [None, None, None];
                args[position - 1] = Some(raw);
                assert!(
                    matches!(
                        RuntimePoolConfig::from_raw(None, args[0], args[1], args[2]),
                        Err(ConfigError::Invalid(reported, _)) if reported == name
                    ),
                    "{name}={raw:?}"
                );
            }
        }
    }

    #[test]
    fn loads_pool_config_from_process_environment() {
        const CASE: &str = "RUNTARA_TEST_RUNTIME_POOL_CASE";
        if let Ok(case) = std::env::var(CASE) {
            let actual = RuntimePoolConfig::from_env();
            match case.as_str() {
                "unset" => assert_eq!(actual.unwrap(), RuntimePoolConfig::default()),
                "set" => assert_eq!(
                    actual.unwrap(),
                    RuntimePoolConfig {
                        max_connections: 64,
                        acquire_timeout: Duration::from_secs(5),
                        idle_timeout: Some(Duration::from_secs(120)),
                        max_lifetime: None,
                    }
                ),
                "invalid" => assert!(matches!(
                    actual,
                    Err(ConfigError::Invalid(POOL_IDLE_TIMEOUT_SECS, _))
                )),
                _ => panic!("unknown test case"),
            }
            return;
        }

        for case in ["unset", "set", "invalid"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "config::runtime::tests::loads_pool_config_from_process_environment",
                    "--nocapture",
                ])
                .env(CASE, case)
                .env_remove(RUNTIME_MAX_CONNECTIONS)
                .env_remove(POOL_ACQUIRE_TIMEOUT_SECS)
                .env_remove(POOL_IDLE_TIMEOUT_SECS)
                .env_remove(POOL_MAX_LIFETIME_SECS);
            if case == "set" {
                child
                    .env(RUNTIME_MAX_CONNECTIONS, "64")
                    .env(POOL_ACQUIRE_TIMEOUT_SECS, "5")
                    .env(POOL_IDLE_TIMEOUT_SECS, "120")
                    .env(POOL_MAX_LIFETIME_SECS, "0");
            } else if case == "invalid" {
                child.env(POOL_IDLE_TIMEOUT_SECS, "ten minutes");
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "{case}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
