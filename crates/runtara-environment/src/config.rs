// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration for runtara-environment.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Environment configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Database URL (shared with Core for checkpoints, events, signals)
    pub database_url: String,
    /// QUIC server address for Environment API
    pub quic_addr: SocketAddr,
    /// Address of Runtara Core (for proxying signals and passing to instances)
    pub core_addr: String,
    /// Data directory for images, bundles, and instance I/O
    pub data_dir: PathBuf,
    /// Skip TLS certificate verification (passed to instances)
    pub skip_cert_verification: bool,
    /// Database connection pool size
    pub db_pool_size: u32,
    /// Request timeout for database operations in milliseconds
    pub db_request_timeout_ms: u64,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        // Shared database with Core for checkpoints, events, signals
        let database_url = std::env::var("RUNTARA_DATABASE_URL")
            .map_err(|_| ConfigError::MissingEnvVar("RUNTARA_DATABASE_URL"))?;

        let port: u16 = std::env::var("RUNTARA_ENV_QUIC_PORT")
            .unwrap_or_else(|_| "8002".to_string())
            .parse()
            .map_err(|_| ConfigError::InvalidPort)?;

        let quic_addr = SocketAddr::from(([0, 0, 0, 0], port));

        let core_addr =
            std::env::var("RUNTARA_CORE_ADDR").unwrap_or_else(|_| "127.0.0.1:8001".to_string());

        let data_dir =
            PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string()));

        let skip_cert_verification = std::env::var("RUNTARA_SKIP_CERT_VERIFICATION")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let db_pool_size = std::env::var("RUNTARA_DB_POOL_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        let db_request_timeout_ms = std::env::var("RUNTARA_DB_REQUEST_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30_000); // 30 seconds default

        Ok(Self {
            database_url,
            quic_addr,
            core_addr,
            data_dir,
            skip_cert_verification,
            db_pool_size,
            db_request_timeout_ms,
        })
    }
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable is missing.
    #[error("Missing required environment variable: {0}")]
    MissingEnvVar(&'static str),
    /// The port number is invalid.
    #[error("Invalid port number")]
    InvalidPort,
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
        guard.remove("RUNTARA_ENV_QUIC_PORT");
        guard.remove("RUNTARA_CORE_ADDR");
        guard.remove("DATA_DIR");
        guard.remove("RUNTARA_SKIP_CERT_VERIFICATION");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "postgres://localhost/test");
        assert_eq!(config.quic_addr.port(), 8002);
        assert_eq!(config.core_addr, "127.0.0.1:8001");
        assert_eq!(config.data_dir, PathBuf::from(".data"));
        assert!(!config.skip_cert_verification);
    }

    #[test]
    fn test_config_from_env_with_custom_port() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_ENV_QUIC_PORT", "9000");

        let config = Config::from_env().unwrap();

        assert_eq!(config.quic_addr.port(), 9000);
    }

    #[test]
    fn test_config_from_env_with_custom_core_addr() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_CORE_ADDR", "10.0.0.5:8080");

        let config = Config::from_env().unwrap();

        assert_eq!(config.core_addr, "10.0.0.5:8080");
    }

    #[test]
    fn test_config_from_env_with_custom_data_dir() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("DATA_DIR", "/var/runtara/data");

        let config = Config::from_env().unwrap();

        assert_eq!(config.data_dir, PathBuf::from("/var/runtara/data"));
    }

    #[test]
    fn test_config_from_env_skip_cert_verification_true() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_SKIP_CERT_VERIFICATION", "true");

        let config = Config::from_env().unwrap();

        assert!(config.skip_cert_verification);
    }

    #[test]
    fn test_config_from_env_skip_cert_verification_one() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_SKIP_CERT_VERIFICATION", "1");

        let config = Config::from_env().unwrap();

        assert!(config.skip_cert_verification);
    }

    #[test]
    fn test_config_from_env_skip_cert_verification_false() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_SKIP_CERT_VERIFICATION", "false");

        let config = Config::from_env().unwrap();

        assert!(!config.skip_cert_verification);
    }

    #[test]
    fn test_config_from_env_all_custom() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://user:pass@db:5432/prod");
        guard.set("RUNTARA_ENV_QUIC_PORT", "8888");
        guard.set("RUNTARA_CORE_ADDR", "192.168.1.100:9001");
        guard.set("DATA_DIR", "/custom/data");
        guard.set("RUNTARA_SKIP_CERT_VERIFICATION", "true");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "postgres://user:pass@db:5432/prod");
        assert_eq!(config.quic_addr.port(), 8888);
        assert_eq!(config.core_addr, "192.168.1.100:9001");
        assert_eq!(config.data_dir, PathBuf::from("/custom/data"));
        assert!(config.skip_cert_verification);
    }

    #[test]
    fn test_config_missing_database_url() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.remove("RUNTARA_DATABASE_URL");

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            err,
            ConfigError::MissingEnvVar("RUNTARA_DATABASE_URL")
        ));
        assert!(err.to_string().contains("RUNTARA_DATABASE_URL"));
    }

    #[test]
    fn test_config_invalid_port() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_ENV_QUIC_PORT", "not_a_number");

        let result = Config::from_env();
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidPort));
    }

    #[test]
    fn test_config_port_out_of_range() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");
        guard.set("RUNTARA_ENV_QUIC_PORT", "99999"); // > 65535

        let result = Config::from_env();
        assert!(result.is_err());
    }

    #[test]
    fn test_config_error_display() {
        let missing = ConfigError::MissingEnvVar("MY_VAR");
        assert_eq!(
            missing.to_string(),
            "Missing required environment variable: MY_VAR"
        );

        let invalid = ConfigError::InvalidPort;
        assert_eq!(invalid.to_string(), "Invalid port number");
    }

    #[test]
    fn test_config_debug() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");

        let config = Config::from_env().unwrap();
        let debug_str = format!("{:?}", config);

        assert!(debug_str.contains("Config"));
        assert!(debug_str.contains("database_url"));
        assert!(debug_str.contains("quic_addr"));
        assert!(debug_str.contains("core_addr"));
        assert!(debug_str.contains("data_dir"));
    }

    #[test]
    fn test_config_clone() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();

        guard.set("RUNTARA_DATABASE_URL", "postgres://localhost/test");

        let config = Config::from_env().unwrap();
        let cloned = config.clone();

        assert_eq!(config.database_url, cloned.database_url);
        assert_eq!(config.quic_addr, cloned.quic_addr);
        assert_eq!(config.core_addr, cloned.core_addr);
        assert_eq!(config.data_dir, cloned.data_dir);
        assert_eq!(config.skip_cert_verification, cloned.skip_cert_verification);
    }
}
