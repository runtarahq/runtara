// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration loading from environment variables.
//!
//! Core is a library, so it reads nothing on its own and owns no defaults for
//! the process it runs in — no storage connection settings, no listen address. What it does
//! own is the pair of knobs that govern the handlers themselves, and
//! [`crate::config::RuntimeOverrides`] hands those to a host in the only shape that keeps the
//! host in charge: `None` where the deployment said nothing.

use std::time::Duration;

/// The core knobs a host can take from the environment, each `None` when its
/// variable is unset.
///
/// A host supplies persistence and its own transport, and must not have a
/// concurrency cap it never asked for switched on underneath it by an upgrade.
/// Optional fields are what separate "unset" from "set to the same value as
/// the default", so a host can apply these over its own defaults and leave
/// untouched whatever the deployment did not mention.
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
    /// is, so a typo in a deployment's configuration surfaces at startup
    /// instead of being silently ignored. What a host does with that error is
    /// its own call: `runtara-environment` exits, while `runtara-server`
    /// reports it and carries on without an embedded runtime.
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
    fn test_config_error_display() {
        let invalid = ConfigError::Invalid("MY_VAR", "must be a number");
        assert_eq!(
            invalid.to_string(),
            "invalid value for MY_VAR: must be a number"
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
