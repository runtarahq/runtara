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
