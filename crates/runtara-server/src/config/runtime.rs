// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Deployment overrides for the server's instance runtime.

use std::time::Duration;

use super::ConfigError;

const MAX_CONCURRENT_INSTANCES: &str = "RUNTARA_MAX_CONCURRENT_INSTANCES";
const SHUTDOWN_GRACE_MS: &str = "RUNTARA_CORE_SHUTDOWN_GRACE_MS";

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
}
