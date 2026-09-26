// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server reencrypt-connections [--tenant-id <id>]`
//!
//! Re-encrypts every stored connection's parameters with the cipher built
//! from `RUNTARA_CONNECTIONS_ENCRYPTION_KEY`. Run it after enabling encryption
//! for the first time or after rotating the key. Idempotent, and safe during
//! live traffic because rows are updated one at a time.

use std::ffi::OsString;

use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::{ENCRYPTION_KEY_ENV, ReencryptionStats, cipher_from_env};
use sqlx::PgPool;

/// The argv[1] that selects this command.
pub const COMMAND: &str = "reencrypt-connections";

const USAGE: &str = "Usage: runtara-server reencrypt-connections [--tenant-id <id>]";

#[derive(Debug, PartialEq, Eq)]
pub struct ReencryptArgs {
    /// Limit the job to one tenant. `None` re-encrypts every tenant.
    pub tenant_id: Option<String>,
}

/// Parse the process arguments. Returns `None` when they don't select this
/// command, so the caller falls through to starting the server.
pub fn parse_args(
    mut args: impl Iterator<Item = OsString>,
) -> Option<Result<ReencryptArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    Some(parse_options(args))
}

fn parse_options(mut args: impl Iterator<Item = OsString>) -> Result<ReencryptArgs, String> {
    let mut tenant_id = None;
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|_| format!("arguments must be valid UTF-8\n{USAGE}"))?;
        let value = match arg.as_str() {
            "--tenant-id" => args
                .next()
                .and_then(|v| v.into_string().ok())
                .ok_or_else(|| format!("--tenant-id needs a value\n{USAGE}"))?,
            other => match other.strip_prefix("--tenant-id=") {
                Some(v) => v.to_string(),
                None => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
            },
        };
        if value.trim().is_empty() {
            return Err(format!("--tenant-id needs a value\n{USAGE}"));
        }
        tenant_id = Some(value);
    }
    Ok(ReencryptArgs { tenant_id })
}

/// Run the job and print its statistics. Errors when encryption is off, the
/// job fails, or any row could not be re-encrypted.
pub async fn run(pool: PgPool, args: ReencryptArgs) -> Result<ReencryptionStats, String> {
    let cipher = cipher_from_env()?;
    if !cipher.is_encrypting() {
        return Err(format!(
            "Encryption is not enabled. Set {ENCRYPTION_KEY_ENV} before running {COMMAND}."
        ));
    }

    let scope = args.tenant_id.as_deref().unwrap_or("all tenants");
    println!("Re-encrypting connection parameters ({scope})...");
    let stats = ConnectionRepository::new(pool, cipher)
        .reencrypt_all(args.tenant_id.as_deref())
        .await
        .map_err(|e| format!("re-encryption failed: {e}"))?;
    println!(
        "scanned={} reencrypted={} unchanged={} failed={}",
        stats.scanned, stats.reencrypted, stats.unchanged, stats.failed
    );

    if stats.failed > 0 {
        return Err(format!(
            "{} connection(s) could not be re-encrypted; see the warnings above",
            stats.failed
        ));
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<ReencryptArgs, String>> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn other_invocations_fall_through_to_the_server() {
        assert_eq!(parse(&["runtara-server"]), None);
        assert_eq!(parse(&["runtara-server", "--normal-server-option"]), None);
    }

    #[test]
    fn tenant_scope_is_optional() {
        assert_eq!(
            parse(&["runtara-server", COMMAND]),
            Some(Ok(ReencryptArgs { tenant_id: None }))
        );
        for args in [
            &["runtara-server", COMMAND, "--tenant-id", "acme"][..],
            &["runtara-server", COMMAND, "--tenant-id=acme"][..],
        ] {
            assert_eq!(
                parse(args),
                Some(Ok(ReencryptArgs {
                    tenant_id: Some("acme".to_string())
                }))
            );
        }
    }

    #[test]
    fn malformed_options_are_rejected() {
        for args in [
            &["runtara-server", COMMAND, "--tenant-id"][..],
            &["runtara-server", COMMAND, "--tenant-id="][..],
            &["runtara-server", COMMAND, "--all"][..],
        ] {
            assert!(matches!(parse(args), Some(Err(_))), "{args:?}");
        }
    }
}
