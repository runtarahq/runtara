// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server object-model set --tenant-id <id> [--if-unset]`
//!
//! Gives a tenant its object model database. The platform supplies none: a
//! tenant's object model, SQL steps and database tools use a Postgres
//! connection of the tenant's, and this makes one and sets it as the
//! tenant's default.
//!
//! The database URL is read from standard input, so that its password is in
//! no argument list or history. It is sealed as it is stored: seal logs in
//! with it, and nothing can read it back. The URL must say how its session
//! is protected (`?sslmode=verify-full`, `require`, or `disable` for a host
//! the deployment lets be reached in the clear).
//!
//! Run again for a tenant that has one, it replaces the URL of that
//! connection where it is (a new password, another database): the
//! connection keeps its id, and nothing is left holding the old URL. With
//! `--if-unset` a tenant that has a default is left as it is, so that a
//! start-up script can run this every time.

use std::ffi::OsString;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::Arc;

use runtara_connections::repository::connections::ConnectionRepository;
use sqlx::PgPool;
use zeroize::Zeroizing;

/// The argv[1] that selects this command.
pub const COMMAND: &str = "object-model";

const USAGE: &str = "Usage: runtara-server object-model set --tenant-id <id> [--if-unset]
       (the database URL is read from standard input)";

#[derive(Debug, PartialEq, Eq)]
pub struct SetArgs {
    /// The tenant whose object model database is set.
    pub tenant_id: String,
    /// Leave a tenant that has one as it is.
    pub if_unset: bool,
}

/// Parse the process arguments. Returns `None` when they don't select this
/// command, so the caller falls through to starting the server.
pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<SetArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    Some(parse_set(args))
}

fn parse_set(mut args: impl Iterator<Item = OsString>) -> Result<SetArgs, String> {
    let text = |arg: Option<OsString>| arg.and_then(|arg| arg.into_string().ok());
    if text(args.next()).as_deref() != Some("set") {
        return Err(USAGE.to_string());
    }
    let (mut tenant_id, mut if_unset) = (None, false);
    while let Some(arg) = args.next() {
        let arg = text(Some(arg)).ok_or_else(|| USAGE.to_string())?;
        match arg.as_str() {
            "--if-unset" => if_unset = true,
            "--tenant-id" => tenant_id = text(args.next()),
            other => match other.strip_prefix("--tenant-id=") {
                Some(tenant) => tenant_id = Some(tenant.to_owned()),
                None => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
            },
        }
    }
    let tenant_id = tenant_id
        .filter(|tenant| !tenant.trim().is_empty())
        .ok_or_else(|| format!("--tenant-id is required\n{USAGE}"))?;
    Ok(SetArgs {
        tenant_id,
        if_unset,
    })
}

/// One line of standard input, without its line ending: the database URL.
fn read_url() -> Result<Zeroizing<String>, String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprint!("Database URL (it is shown as you type; pipe it in to avoid that): ");
        let _ = std::io::stderr().flush();
    }
    let mut line = Zeroizing::new(String::new());
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("the database URL could not be read: {error}"))?;
    let url = Zeroizing::new(line.trim().to_owned());
    if url.is_empty() {
        return Err("no database URL was given on standard input".to_string());
    }
    Ok(url)
}

/// Read the URL and make it the tenant's default object model database.
pub async fn run(
    pool: PgPool,
    args: SetArgs,
    cipher: Option<Arc<dyn runtara_connections::CredentialCipher>>,
) -> Result<(), String> {
    if args.if_unset
        && let Some(connection_id) = default_of(&pool, &args.tenant_id).await?
    {
        println!(
            "Tenant '{}' already uses connection {connection_id} as its object model database.",
            args.tenant_id
        );
        return Ok(());
    }
    let url = read_url()?;
    let connection_id = set(pool, &args.tenant_id, &url, cipher).await?;
    println!(
        "Tenant '{}' uses connection {connection_id} as its object model database.",
        args.tenant_id
    );
    Ok(())
}

/// The connection that is the tenant's default object model database.
async fn default_of(pool: &PgPool, tenant_id: &str) -> Result<Option<String>, String> {
    sqlx::query_scalar(
        "SELECT connection_id FROM connection_defaults WHERE tenant_id = $1 AND default_for = $2",
    )
    .bind(tenant_id)
    .bind(crate::api::services::object_model::OBJECT_MODEL_DEFAULT_FOR)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("the tenant's default could not be read: {error}"))
}

/// Set the tenant's default object model database to `database_url`, sealed:
/// in the tenant's default Postgres connection if it has one, else in a new
/// one. Returns the connection's id. The
/// error never includes the URL.
pub async fn set(
    pool: PgPool,
    tenant_id: &str,
    database_url: &str,
    cipher: Option<Arc<dyn runtara_connections::CredentialCipher>>,
) -> Result<String, String> {
    if !["postgres://", "postgresql://"]
        .iter()
        .any(|scheme| database_url.starts_with(scheme))
    {
        return Err("the database URL must start with postgres:// or postgresql://".to_string());
    }
    let cipher = match cipher {
        Some(cipher) => cipher,
        None => runtara_connections::cipher_from_env()?,
    };
    // seal as the server builds it, so that the URL is sealed here as a
    // connection saved there would be: with the server's configuration.
    if crate::config::try_get().is_none() {
        crate::config::init(
            crate::config::Config::from_env()
                .map_err(|error| format!("the server's configuration is not valid: {error}"))?,
        );
    }
    let sealed = runtara_connections::sealed::from_env(pool.clone(), crate::seal_allowances())?;
    runtara_connections::facade::set_default_connection(
        &ConnectionRepository::new(pool, cipher),
        &sealed,
        tenant_id,
        crate::api::services::object_model::OBJECT_MODEL_DEFAULT_FOR,
        "Object Model Database".to_string(),
        "postgres".to_string(),
        serde_json::json!({ "database_url": database_url }),
    )
    .await
    .map_err(|error| format!("the database was not set: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<SetArgs, String>> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn other_invocations_fall_through_to_the_server() {
        assert_eq!(parse(&["runtara-server"]), None);
        assert_eq!(parse(&["runtara-server", "oauth-apps", "list"]), None);
    }

    #[test]
    fn the_tenant_is_named_and_the_url_is_not_an_argument() {
        for args in [
            &["runtara-server", COMMAND, "set", "--tenant-id", "acme"][..],
            &["runtara-server", COMMAND, "set", "--tenant-id=acme"][..],
        ] {
            assert_eq!(
                parse(args),
                Some(Ok(SetArgs {
                    tenant_id: "acme".to_string(),
                    if_unset: false,
                }))
            );
        }
        assert_eq!(
            parse(&[
                "runtara-server",
                COMMAND,
                "set",
                "--if-unset",
                "--tenant-id=acme"
            ]),
            Some(Ok(SetArgs {
                tenant_id: "acme".to_string(),
                if_unset: true,
            }))
        );
        for bad in [
            &["runtara-server", COMMAND][..],
            &["runtara-server", COMMAND, "set"][..],
            &["runtara-server", COMMAND, "set", "--tenant-id"][..],
            &["runtara-server", COMMAND, "set", "--tenant-id="][..],
            &["runtara-server", COMMAND, "show", "--tenant-id", "acme"][..],
            &[
                "runtara-server",
                COMMAND,
                "set",
                "--tenant-id",
                "acme",
                "postgres://u:p@h/d",
            ][..],
        ] {
            assert!(matches!(parse(bad), Some(Err(_))), "{bad:?}");
        }
    }
}
