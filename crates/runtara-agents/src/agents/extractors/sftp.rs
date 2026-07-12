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
    category = "file_storage",
    auth_type = "ssh_key"
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

    /// Authentication mode used by the schema-driven editor.
    #[serde(default = "default_auth_mode")]
    #[field(
        display_name = "Authentication Mode",
        description = "Choose password or private-key authentication.",
        default = "password",
        enum_values = "password,private_key"
    )]
    auth_mode: String,

    /// Password for authentication (optional if using private key)
    #[serde(default)]
    #[field(
        display_name = "Password",
        description = "Password for authentication (optional if using private key)",
        secret,
        clearable,
        visible = sftp_auth_is_password,
        required = sftp_auth_is_password
    )]
    password: Option<String>,

    /// Private key for authentication (PEM format)
    #[serde(default)]
    #[field(
        display_name = "Private Key",
        description = "Private key in PEM format (optional if using password)",
        secret,
        clearable,
        control = "secret_textarea",
        visible = sftp_auth_is_private_key,
        required = sftp_auth_is_private_key
    )]
    private_key: Option<String>,

    /// Passphrase for private key
    #[serde(default)]
    #[field(
        display_name = "Passphrase",
        description = "Passphrase for the private key (if encrypted)",
        secret,
        clearable,
        visible = sftp_auth_is_private_key
    )]
    passphrase: Option<String>,
}

#[allow(dead_code)]
fn default_port() -> u16 {
    22
}

#[allow(dead_code)]
fn default_auth_mode() -> String {
    "password".to_string()
}

fn sftp_auth_is_password() -> runtara_dsl::ConditionExpression {
    runtara_dsl::form::not(sftp_auth_is_private_key())
}

fn sftp_auth_is_private_key() -> runtara_dsl::ConditionExpression {
    runtara_dsl::form::any([
        runtara_dsl::form::field_equals("auth_mode", "private_key"),
        runtara_dsl::form::all([
            runtara_dsl::form::not(runtara_dsl::form::field_is_defined("auth_mode")),
            runtara_dsl::form::field_is_defined("private_key"),
        ]),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::form::{analyze_form, connection_form_definition};
    use serde_json::json;

    #[test]
    fn generated_form_switches_password_and_private_key_fields() {
        let definition = connection_form_definition(&__CONNECTION_META_SftpParams);

        let password = analyze_form(
            &definition,
            &json!({
                "host": "sftp.example.com",
                "port": 22,
                "username": "demo",
                "auth_mode": "password"
            }),
        );
        assert!(password.fields["password"].visible);
        assert!(password.fields["password"].required);
        assert!(!password.fields["private_key"].visible);
        assert!(!password.fields["passphrase"].visible);
        assert!(!password.valid);

        let private_key = analyze_form(
            &definition,
            &json!({
                "host": "sftp.example.com",
                "port": 22,
                "username": "demo",
                "auth_mode": "private_key",
                "private_key": "-----BEGIN PRIVATE KEY-----"
            }),
        );
        assert!(!private_key.fields["password"].visible);
        assert!(private_key.fields["private_key"].visible);
        assert!(private_key.fields["private_key"].required);
        assert!(private_key.fields["passphrase"].visible);
        assert!(private_key.valid);

        let legacy_private_key = analyze_form(
            &definition,
            &json!({
                "host": "sftp.example.com",
                "port": 22,
                "username": "demo",
                "private_key": "-----BEGIN PRIVATE KEY-----"
            }),
        );
        assert!(!legacy_private_key.fields["password"].visible);
        assert!(legacy_private_key.fields["private_key"].visible);
        assert!(legacy_private_key.valid);
        assert!(
            __CONNECTION_META_SftpParams
                .fields
                .iter()
                .filter(|field| field.is_secret)
                .all(|field| field.behavior.clearable)
        );
    }
}
