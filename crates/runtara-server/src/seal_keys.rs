// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server seal-keys export --out <dir> [--wrapping-key-out <file>]`
//! `runtara-server seal-keys forget --confirm`
//!
//! Moves tenants' seal keys from this deployment's database to a seal that
//! runs as a process of its own (`RUNTARA_SEAL_MODE=unix|mtls`).
//!
//! `export` writes the keys, still wrapped, as seal's key directory holds
//! them, and, if asked, the key that wraps them as seal reads a wrapping key
//! file. Both go to seal's host (the directory as it is, or copied into the
//! bucket seal reads), and the wrapping key leaves this server's environment.
//!
//! `forget`, run by a server that is told of that seal, deletes the keys from
//! the database once seal answers for every one of them: a copy left there is
//! a copy the wrapping key still opens.

use std::ffi::OsString;
use std::path::PathBuf;

use sqlx::PgPool;

/// The argv[1] that selects this command.
pub const COMMAND: &str = "seal-keys";

const USAGE: &str = "Usage: runtara-server seal-keys export --out <dir> [--wrapping-key-out <file>]\n       runtara-server seal-keys forget --confirm";

#[derive(Debug, PartialEq, Eq)]
pub enum KeysArgs {
    /// Write the keys into `out`, and the wrapping key to `wrapping_key_out`.
    Export {
        out: PathBuf,
        wrapping_key_out: Option<PathBuf>,
    },
    /// Delete from the database the keys seal holds.
    Forget,
}

/// Parse the process arguments. Returns `None` when they don't select this
/// command, so the caller falls through to starting the server.
pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<KeysArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    Some(parse_options(args))
}

fn parse_options(mut args: impl Iterator<Item = OsString>) -> Result<KeysArgs, String> {
    let text = |arg: OsString| {
        arg.into_string()
            .map_err(|_| format!("arguments must be valid UTF-8\n{USAGE}"))
    };
    let action = match args.next() {
        Some(action) => text(action)?,
        None => return Err(format!("say what to do with the keys\n{USAGE}")),
    };
    match action.as_str() {
        "export" => {
            let (mut out, mut wrapping_key_out) = (None, None);
            while let Some(arg) = args.next() {
                let arg = text(arg)?;
                let (name, inline) = match arg.split_once('=') {
                    Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
                    None => (arg, None),
                };
                let slot = match name.as_str() {
                    "--out" => &mut out,
                    "--wrapping-key-out" => &mut wrapping_key_out,
                    other => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
                };
                let value = match inline {
                    Some(value) => value,
                    None => match args.next() {
                        Some(value) => text(value)?,
                        None => return Err(format!("{name} needs a value\n{USAGE}")),
                    },
                };
                if value.trim().is_empty() {
                    return Err(format!("{name} needs a value\n{USAGE}"));
                }
                *slot = Some(PathBuf::from(value));
            }
            Ok(KeysArgs::Export {
                out: out.ok_or_else(|| format!("--out is required\n{USAGE}"))?,
                wrapping_key_out,
            })
        }
        "forget" => {
            let mut confirmed = false;
            for arg in args {
                match text(arg)?.as_str() {
                    "--confirm" => confirmed = true,
                    other => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
                }
            }
            if !confirmed {
                return Err(format!(
                    "--confirm is required: the keys are deleted from this database, and only \
                     seal then holds them\n{USAGE}"
                ));
            }
            Ok(KeysArgs::Forget)
        }
        other => Err(format!("unexpected action '{other}'\n{USAGE}")),
    }
}

/// Run the command.
pub async fn run(pool: PgPool, args: KeysArgs) -> Result<(), String> {
    use runtara_connections::sealed;
    match args {
        KeysArgs::Export {
            out,
            wrapping_key_out,
        } => {
            // The key is worked out before anything is written, so that a
            // missing one leaves no half of an export behind.
            let wrapping_key = match &wrapping_key_out {
                Some(_) => Some(sealed::wrapping_key(
                    std::env::var(sealed::WRAPPING_KEY_ENV).ok().as_deref(),
                    std::env::var(runtara_connections::crypto::ENCRYPTION_KEY_ENV)
                        .ok()
                        .as_deref(),
                )?),
                None => None,
            };
            let (written, present) = sealed::keys::export_keys(&pool, &out).await?;
            println!(
                "Wrote {written} seal key(s) to {} ({present} already there).",
                out.display()
            );
            if let (Some(path), Some(key)) = (&wrapping_key_out, &wrapping_key) {
                sealed::keys::export_wrapping_key(path, key)?;
                println!(
                    "Wrote the key that wraps them to {}. It opens every seal key: move it to \
                     seal's host and leave no copy.",
                    path.display()
                );
            }
            println!(
                "Give seal the directory (keys.source) and the wrapping key (keys.wrap), start \
                 this server with {}, and then run `seal-keys forget --confirm`.",
                sealed::mode::MODE_ENV
            );
            Ok(())
        }
        KeysArgs::Forget => {
            let seal = sealed::ready_from_env(pool.clone(), crate::seal_allowances()).await?;
            let forgotten = sealed::forget_keys_seal_holds(&pool, &seal).await?;
            println!(
                "Deleted {forgotten} seal key(s) from this database: seal ({}) holds them.",
                seal.mode()
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<KeysArgs, String>> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn other_invocations_fall_through_to_the_server() {
        assert_eq!(parse(&["runtara-server"]), None);
        assert_eq!(parse(&["runtara-server", "destroy-seal-keys"]), None);
    }

    #[test]
    fn an_export_names_where_the_keys_go() {
        assert_eq!(
            parse(&["runtara-server", "seal-keys", "export", "--out", "/k"]),
            Some(Ok(KeysArgs::Export {
                out: "/k".into(),
                wrapping_key_out: None
            }))
        );
        assert_eq!(
            parse(&[
                "runtara-server",
                "seal-keys",
                "export",
                "--out=/k",
                "--wrapping-key-out",
                "/kek"
            ]),
            Some(Ok(KeysArgs::Export {
                out: "/k".into(),
                wrapping_key_out: Some("/kek".into())
            }))
        );
        for bad in [
            &["runtara-server", "seal-keys", "export"][..],
            &["runtara-server", "seal-keys", "export", "--out"],
            &["runtara-server", "seal-keys", "export", "--out", " "],
            &[
                "runtara-server",
                "seal-keys",
                "export",
                "--out",
                "/k",
                "--to",
                "x",
            ],
        ] {
            assert!(matches!(parse(bad), Some(Err(_))), "{bad:?}");
        }
    }

    #[test]
    fn forgetting_the_keys_is_confirmed() {
        assert_eq!(
            parse(&["runtara-server", "seal-keys", "forget", "--confirm"]),
            Some(Ok(KeysArgs::Forget))
        );
        let unconfirmed = parse(&["runtara-server", "seal-keys", "forget"]).unwrap();
        assert!(unconfirmed.unwrap_err().contains("--confirm"));
    }

    #[test]
    fn an_action_is_required_and_known() {
        assert!(matches!(
            parse(&["runtara-server", "seal-keys"]),
            Some(Err(_))
        ));
        assert!(matches!(
            parse(&["runtara-server", "seal-keys", "rotate"]),
            Some(Err(_))
        ));
    }
}
