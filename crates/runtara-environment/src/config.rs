// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration helpers for runtara-environment.
//!
//! The crate is a library and reads no configuration of its own — a host builds
//! an [`EnvironmentRuntime`](crate::runtime::EnvironmentRuntime) from its own
//! settings. What lives here is the lenient boolean parser its callers share,
//! and the error type they report with.

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

/// Parse a boolean env var accepting the common forms: `true/false`, `1/0`,
/// `yes/no`, `on/off` (case-insensitive). Unknown values are treated as `false`.
pub(crate) fn parse_bool_lenient(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
