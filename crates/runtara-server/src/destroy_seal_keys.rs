// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server destroy-seal-keys --tenant-id <id> --confirm <id>`
//!
//! Destroys a tenant's seal keys, for a tenant that is leaving. Every secret
//! sealed for that tenant, in this database and in every copy of its rows,
//! can then no longer be opened: its connections stop working, and nothing
//! brings them back. A backup of the `seal_wrapped_key` table still opens
//! them while the wrapping key exists, so a backup that must not is deleted
//! separately.
//!
//! The tenant is named twice, as `--tenant-id` and as `--confirm`, because
//! this cannot be undone.

use std::ffi::OsString;

use runtara_connections::sealed::SEALED_CREDENTIALS_ENV;
use sqlx::PgPool;

/// The argv[1] that selects this command.
pub const COMMAND: &str = "destroy-seal-keys";

const USAGE: &str = "Usage: runtara-server destroy-seal-keys --tenant-id <id> --confirm <id>";

#[derive(Debug, PartialEq, Eq)]
pub struct DestroyArgs {
    /// The tenant whose keys are destroyed.
    pub tenant_id: String,
}

/// Parse the process arguments. Returns `None` when they don't select this
/// command, so the caller falls through to starting the server.
pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<DestroyArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    Some(parse_options(args))
}

fn parse_options(mut args: impl Iterator<Item = OsString>) -> Result<DestroyArgs, String> {
    let (mut tenant_id, mut confirm) = (None, None);
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|_| format!("arguments must be valid UTF-8\n{USAGE}"))?;
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (arg, None),
        };
        let slot = match name.as_str() {
            "--tenant-id" => &mut tenant_id,
            "--confirm" => &mut confirm,
            other => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
        };
        let value = match inline {
            Some(value) => value,
            None => args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("{name} needs a value\n{USAGE}"))?,
        };
        if value.trim().is_empty() {
            return Err(format!("{name} needs a value\n{USAGE}"));
        }
        *slot = Some(value);
    }
    let tenant_id = tenant_id.ok_or_else(|| format!("--tenant-id is required\n{USAGE}"))?;
    match confirm {
        Some(confirm) if confirm == tenant_id => Ok(DestroyArgs { tenant_id }),
        Some(_) => Err(format!(
            "--confirm must name the same tenant as --tenant-id\n{USAGE}"
        )),
        None => Err(format!(
            "--confirm {tenant_id} is required: destroying a tenant's keys cannot be undone\n{USAGE}"
        )),
    }
}

/// Destroy the tenant's keys. Errors when sealing is off or the keys could
/// not be deleted.
pub async fn run(pool: PgPool, args: DestroyArgs) -> Result<(), String> {
    let sealed = runtara_connections::sealed::from_env(pool, crate::seal_allowances())?
        .ok_or_else(|| {
            format!(
                "Sealed credentials are not enabled. Set {SEALED_CREDENTIALS_ENV}=1 before \
                 running {COMMAND}."
            )
        })?;
    println!("Destroying the seal keys of tenant '{}'...", args.tenant_id);
    sealed
        .destroy_tenant_keys(&args.tenant_id)
        .await
        .map_err(|error| format!("the keys were not destroyed: {error}"))?;
    println!(
        "Destroyed. Secrets sealed for '{}' can no longer be opened.",
        args.tenant_id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<DestroyArgs, String>> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn other_invocations_fall_through_to_the_server() {
        assert_eq!(parse(&["runtara-server"]), None);
        assert_eq!(parse(&["runtara-server", "seal-connections"]), None);
    }

    #[test]
    fn the_tenant_is_named_twice() {
        for args in [
            &[
                "runtara-server",
                COMMAND,
                "--tenant-id",
                "acme",
                "--confirm",
                "acme",
            ][..],
            &[
                "runtara-server",
                COMMAND,
                "--confirm=acme",
                "--tenant-id=acme",
            ][..],
        ] {
            assert_eq!(
                parse(args),
                Some(Ok(DestroyArgs {
                    tenant_id: "acme".to_string()
                }))
            );
        }
    }

    #[test]
    fn anything_short_of_that_destroys_nothing() {
        for args in [
            &["runtara-server", COMMAND][..],
            &["runtara-server", COMMAND, "--tenant-id", "acme"][..],
            &["runtara-server", COMMAND, "--confirm", "acme"][..],
            &[
                "runtara-server",
                COMMAND,
                "--tenant-id",
                "acme",
                "--confirm",
                "other",
            ][..],
            &[
                "runtara-server",
                COMMAND,
                "--tenant-id",
                "acme",
                "--confirm",
            ][..],
            &["runtara-server", COMMAND, "--tenant-id=", "--confirm="][..],
            &[
                "runtara-server",
                COMMAND,
                "--tenant-id",
                "acme",
                "--confirm",
                "acme",
                "--all",
            ][..],
        ] {
            assert!(matches!(parse(args), Some(Err(_))), "{args:?}");
        }
    }
}
