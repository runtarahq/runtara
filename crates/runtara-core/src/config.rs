// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration loading from environment variables.

use std::net::SocketAddr;

/// Runtara Core configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL or SQLite connection URL
    pub database_url: String,
    /// QUIC server address for instance communication
    pub quic_addr: SocketAddr,
    /// Maximum concurrent instances
    pub max_concurrent_instances: u32,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Required:
    /// - `RUNTARA_DATABASE_URL`: PostgreSQL or SQLite connection string
    ///
    /// Optional (with defaults):
    /// - `RUNTARA_QUIC_PORT`: QUIC server port (default: 8001)
    /// - `RUNTARA_MAX_CONCURRENT_INSTANCES`: Max concurrent instances (default: 32)
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = std::env::var("RUNTARA_DATABASE_URL")
            .map_err(|_| ConfigError::Missing("RUNTARA_DATABASE_URL"))?;

        let quic_port: u16 = std::env::var("RUNTARA_QUIC_PORT")
            .unwrap_or_else(|_| "8001".to_string())
            .parse()
            .map_err(|_| {
                ConfigError::Invalid("RUNTARA_QUIC_PORT", "must be a valid port number")
            })?;

        let max_concurrent_instances: u32 = std::env::var("RUNTARA_MAX_CONCURRENT_INSTANCES")
            .unwrap_or_else(|_| "32".to_string())
            .parse()
            .map_err(|_| {
                ConfigError::Invalid(
                    "RUNTARA_MAX_CONCURRENT_INSTANCES",
                    "must be a positive integer",
                )
            })?;

        Ok(Self {
            database_url,
            quic_addr: SocketAddr::from(([0, 0, 0, 0], quic_port)),
            max_concurrent_instances,
        })
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
        guard.remove("RUNTARA_QUIC_PORT");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "postgres://localhost/test");
        assert_eq!(config.quic_addr.port(), 8001);
        assert_eq!(config.max_concurrent_instances, 32);
    }

    #[test]
    fn test_config_from_env_with_custom_port() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "sqlite:test.db");
        guard.set("RUNTARA_QUIC_PORT", "9999");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "sqlite:test.db");
        assert_eq!(config.quic_addr.port(), 9999);
        assert_eq!(config.max_concurrent_instances, 32);
    }

    #[test]
    fn test_config_from_env_with_custom_max_instances() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.remove("RUNTARA_QUIC_PORT");
        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "100");

        let config = Config::from_env().unwrap();

        assert_eq!(config.max_concurrent_instances, 100);
    }

    #[test]
    fn test_config_from_env_all_custom() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://user:pass@db:5432/prod");
        guard.set("RUNTARA_QUIC_PORT", "8080");
        guard.set("RUNTARA_MAX_CONCURRENT_INSTANCES", "256");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "postgres://user:pass@db:5432/prod");
        assert_eq!(config.quic_addr.port(), 8080);
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
    fn test_config_invalid_quic_port() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_QUIC_PORT", "not_a_number");

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid("RUNTARA_QUIC_PORT", _)
        ));
    }

    #[test]
    fn test_config_invalid_quic_port_out_of_range() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_QUIC_PORT", "99999"); // > 65535

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid("RUNTARA_QUIC_PORT", _)
        ));
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
        guard.remove("RUNTARA_QUIC_PORT");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("database_url"));
        assert!(debug_str.contains("quic_addr"));
    }

    #[test]
    fn test_config_clone() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.remove("RUNTARA_QUIC_PORT");
        guard.remove("RUNTARA_MAX_CONCURRENT_INSTANCES");

        let config = Config::from_env().unwrap();
        let cloned = config.clone();

        assert_eq!(config.database_url, cloned.database_url);
        assert_eq!(config.quic_addr, cloned.quic_addr);
        assert_eq!(
            config.max_concurrent_instances,
            cloned.max_concurrent_instances
        );
    }
}
