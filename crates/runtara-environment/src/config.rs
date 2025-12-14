// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration for runtara-environment.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Environment configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Database URL for runtara-environment (separate from Core)
    pub database_url: String,
    /// QUIC server address for Environment API
    pub quic_addr: SocketAddr,
    /// Address of Runtara Core (for proxying signals and passing to instances)
    pub core_addr: String,
    /// Data directory for images, bundles, and instance I/O
    pub data_dir: PathBuf,
    /// Skip TLS certificate verification (passed to instances)
    pub skip_cert_verification: bool,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        // Environment has its own database, separate from Core
        let database_url = std::env::var("RUNTARA_ENVIRONMENT_DATABASE_URL")
            .or_else(|_| std::env::var("RUNTARA_DATABASE_URL"))
            .map_err(|_| {
                ConfigError::MissingEnvVar(
                    "RUNTARA_ENVIRONMENT_DATABASE_URL or RUNTARA_DATABASE_URL",
                )
            })?;

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

        Ok(Self {
            database_url,
            quic_addr,
            core_addr,
            data_dir,
            skip_cert_verification,
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
