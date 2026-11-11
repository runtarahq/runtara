// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server seal-connections [--tenant-id <id>] [--dry-run]`
//!
//! Seals the secrets of connections stored before sealed credentials were
//! turned on (`RUNTARA_SEALED_CREDENTIALS`): each is sealed as it would be on
//! its next save. A connection of a type seal does not handle, or whose
//! secret goes somewhere seal cannot express, is left as it is.
//!
//! Sealing is one-way: a sealed secret is not stored readable again. Run with
//! `--dry-run` first to see how many connections it would seal. Idempotent,
//! and safe during live traffic: rows are updated one at a time, and a row
//! written meanwhile is left to that write.

use std::ffi::OsString;

use runtara_connections::repository::connections::{ConnectionRepository, SealingStats};
use runtara_connections::sealed::SEALED_CREDENTIALS_ENV;
use runtara_connections::{ENCRYPTION_KEY_ENV, cipher_from_env};
use sqlx::PgPool;

/// The argv[1] that selects this command.
pub const COMMAND: &str = "seal-connections";

const USAGE: &str = "Usage: runtara-server seal-connections [--tenant-id <id>] [--dry-run]";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SealArgs {
    /// Limit the job to one tenant. `None` seals every tenant's connections.
    pub tenant_id: Option<String>,
    /// Report what would be sealed, and write nothing.
    pub dry_run: bool,
}

/// Parse the process arguments. Returns `None` when they don't select this
/// command, so the caller falls through to starting the server.
pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<SealArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    Some(parse_options(args))
}

fn parse_options(mut args: impl Iterator<Item = OsString>) -> Result<SealArgs, String> {
    let mut parsed = SealArgs::default();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|_| format!("arguments must be valid UTF-8\n{USAGE}"))?;
        let tenant = match arg.as_str() {
            "--dry-run" => {
                parsed.dry_run = true;
                continue;
            }
            "--tenant-id" => args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("--tenant-id needs a value\n{USAGE}"))?,
            other => match other.strip_prefix("--tenant-id=") {
                Some(value) => value.to_string(),
                None => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
            },
        };
        if tenant.trim().is_empty() {
            return Err(format!("--tenant-id needs a value\n{USAGE}"));
        }
        parsed.tenant_id = Some(tenant);
    }
    Ok(parsed)
}

/// Run the job and print its statistics, with the host's cipher if it
/// supplies one (else the keyring from the environment). Errors when sealing
/// is off, the job fails, or any connection could not be sealed.
pub async fn run(
    pool: PgPool,
    args: SealArgs,
    cipher: Option<std::sync::Arc<dyn runtara_connections::CredentialCipher>>,
) -> Result<SealingStats, String> {
    let cipher = match cipher {
        Some(cipher) => cipher,
        None => cipher_from_env()?,
    };
    if !cipher.is_encrypting() {
        return Err(format!(
            "Encryption is not enabled. Set {ENCRYPTION_KEY_ENV} before running {COMMAND}."
        ));
    }
    // seal as the server builds it, so that a connection is sealed here as
    // it would be on its next save there: with the server's configuration.
    if crate::config::try_get().is_none() {
        crate::config::init(
            crate::config::Config::from_env()
                .map_err(|error| format!("the server's configuration is not valid: {error}"))?,
        );
    }
    let sealed = runtara_connections::sealed::from_env(pool.clone(), crate::seal_allowances())?
        .ok_or_else(|| {
            format!(
                "Sealed credentials are not enabled. Set {SEALED_CREDENTIALS_ENV}=1 before \
                 running {COMMAND}."
            )
        })?;

    let scope = args.tenant_id.as_deref().unwrap_or("all tenants");
    let action = if args.dry_run {
        "Counting connections to seal"
    } else {
        "Sealing connections"
    };
    println!("{action} ({scope})...");
    let stats = ConnectionRepository::new(pool, cipher)
        .seal_existing(&sealed, args.tenant_id.as_deref(), args.dry_run)
        .await
        .map_err(|error| {
            format!(
                "sealing failed: {error}\n\
                 If the schema is from an earlier release, start the server once to migrate it."
            )
        })?;
    let sealed_label = if args.dry_run { "would_seal" } else { "sealed" };
    println!(
        "scanned={} {sealed_label}={} unchanged={} failed={}",
        stats.scanned, stats.sealed, stats.unchanged, stats.failed
    );
    if stats.failed > 0 {
        return Err(format!(
            "{} connection(s) could not be sealed; see the errors above",
            stats.failed
        ));
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<SealArgs, String>> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn other_invocations_fall_through_to_the_server() {
        assert_eq!(parse(&["runtara-server"]), None);
        assert_eq!(parse(&["runtara-server", "reencrypt-connections"]), None);
        assert_eq!(parse(&["runtara-server", "--normal-server-option"]), None);
    }

    #[test]
    fn a_tenant_and_a_dry_run_are_optional() {
        assert_eq!(
            parse(&["runtara-server", COMMAND]),
            Some(Ok(SealArgs::default()))
        );
        assert_eq!(
            parse(&["runtara-server", COMMAND, "--dry-run"]),
            Some(Ok(SealArgs {
                tenant_id: None,
                dry_run: true
            }))
        );
        for args in [
            &[
                "runtara-server",
                COMMAND,
                "--tenant-id",
                "acme",
                "--dry-run",
            ][..],
            &["runtara-server", COMMAND, "--dry-run", "--tenant-id=acme"][..],
        ] {
            assert_eq!(
                parse(args),
                Some(Ok(SealArgs {
                    tenant_id: Some("acme".to_string()),
                    dry_run: true
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
