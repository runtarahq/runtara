// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SFTP connection type registration
//!
//! This module registers the SFTP connection type for the connection types API.
//! Note: SFTP doesn't use the HttpConnectionExtractor - this is just for schema registration.

use runtara_agent_macro::ConnectionParams;
use serde::Deserialize;

/// Parameters for SFTP connection
#[allow(dead_code)]
#[derive(Debug, Deserialize, ConnectionParams)]
#[connection(
    integration_id = "sftp",
    display_name = "SFTP",
    description = "Connect to an SFTP server for file operations",
    category = "file_storage"
)]
pub struct SftpParams {
    /// SFTP server hostname or IP
    #[field(
        display_name = "Host",
        description = "SFTP server hostname or IP address",
        placeholder = "sftp.example.com"
    )]
    host: String,

    /// SFTP server port
    #[serde(default = "default_port")]
    #[field(
        display_name = "Port",
        description = "SFTP server port",
        default = "22"
    )]
    port: u16,

    /// Username for authentication
    #[field(
        display_name = "Username",
        description = "Username for SFTP authentication"
    )]
    username: String,

    /// Password for authentication (optional if using private key)
    #[serde(default)]
    #[field(
        display_name = "Password",
        description = "Password for authentication (optional if using private key)",
        secret
    )]
    password: Option<String>,

    /// Private key for authentication (PEM format)
    #[serde(default)]
    #[field(
        display_name = "Private Key",
        description = "Private key in PEM format (optional if using password)",
        secret
    )]
    private_key: Option<String>,

    /// Passphrase for private key
    #[serde(default)]
    #[field(
        display_name = "Passphrase",
        description = "Passphrase for the private key (if encrypted)",
        secret
    )]
    passphrase: Option<String>,
}

#[allow(dead_code)]
fn default_port() -> u16 {
    22
}
