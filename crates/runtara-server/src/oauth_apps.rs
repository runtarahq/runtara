// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server oauth-apps <put|list|remove>`
//!
//! The platform's OAuth apps: those it registered with a provider, for
//! tenants to connect with instead of registering their own. They are kept
//! in the database with each client secret sealed by seal, to be sent only
//! to that app's token endpoint. A secret is entered here once and can never
//! be read back: to change one, enter the app again.
//!
//! - `put` stores an app, in place of any with its id. The client secret is
//!   read from standard input, so that it is in no argument list or history.
//! - `list` names the stored apps. No secrets.
//! - `remove` deletes one. Connections on it stop working.
//!
//! A running server sees a change within a minute.

use std::ffi::OsString;
use std::io::{BufRead, IsTerminal, Write};

use runtara_connections::NewOAuthApp;
use runtara_connections::oauth_apps;
use sqlx::PgPool;
use zeroize::Zeroizing;

/// The argv[1] that selects this command.
pub const COMMAND: &str = "oauth-apps";

const USAGE: &str = "Usage:
  runtara-server oauth-apps put --id <id> --integration <connection type> --client-id <id> \\
      --scopes \"<scopes>\" [--required-scopes \"<scopes>\"] [--display-name <name>] \\
      [--parameters '<json object>']        (the client secret is read from standard input)
  runtara-server oauth-apps list
  runtara-server oauth-apps remove --id <id>";

#[derive(Debug, PartialEq, Eq)]
pub enum AppsArgs {
    Put(NewOAuthApp),
    List,
    Remove { id: String },
}

/// Parse the process arguments. Returns `None` when they don't select this
/// command, so the caller falls through to starting the server.
pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<AppsArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    Some(parse_action(args))
}

fn parse_action(mut args: impl Iterator<Item = OsString>) -> Result<AppsArgs, String> {
    let action = args
        .next()
        .and_then(|action| action.into_string().ok())
        .ok_or_else(|| USAGE.to_string())?;
    match action.as_str() {
        "list" => match args.next() {
            None => Ok(AppsArgs::List),
            Some(_) => Err(format!("list takes no arguments\n{USAGE}")),
        },
        "put" => {
            let mut options = parse_options(
                args,
                &[
                    "--id",
                    "--integration",
                    "--client-id",
                    "--scopes",
                    "--required-scopes",
                    "--display-name",
                    "--parameters",
                ],
            )?;
            let mut required = |name: &str| {
                options
                    .remove(name)
                    .ok_or_else(|| format!("{name} is required\n{USAGE}"))
            };
            let id = required("--id")?;
            let integration_id = required("--integration")?;
            let client_id = required("--client-id")?;
            let scopes = required("--scopes")?;
            let parameters = match options.remove("--parameters") {
                Some(json) => serde_json::from_str::<serde_json::Value>(&json)
                    .ok()
                    .filter(serde_json::Value::is_object)
                    .ok_or_else(|| format!("--parameters must be a JSON object\n{USAGE}"))?,
                None => serde_json::json!({}),
            };
            Ok(AppsArgs::Put(NewOAuthApp {
                display_name: options
                    .remove("--display-name")
                    .unwrap_or_else(|| id.clone()),
                required_scopes: options.remove("--required-scopes").unwrap_or_default(),
                id,
                integration_id,
                client_id,
                scopes,
                parameters,
            }))
        }
        "remove" => {
            let mut options = parse_options(args, &["--id"])?;
            let id = options
                .remove("--id")
                .ok_or_else(|| format!("--id is required\n{USAGE}"))?;
            Ok(AppsArgs::Remove { id })
        }
        other => Err(format!("unknown action '{other}'\n{USAGE}")),
    }
}

/// `--name value` and `--name=value` pairs, each of `allowed` at most once.
fn parse_options(
    mut args: impl Iterator<Item = OsString>,
    allowed: &[&str],
) -> Result<std::collections::HashMap<String, String>, String> {
    let mut options = std::collections::HashMap::new();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|_| format!("arguments must be valid UTF-8\n{USAGE}"))?;
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (arg, None),
        };
        if !allowed.contains(&name.as_str()) {
            return Err(format!("unexpected argument '{name}'\n{USAGE}"));
        }
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
        if options.insert(name.clone(), value).is_some() {
            return Err(format!("{name} is given twice\n{USAGE}"));
        }
    }
    Ok(options)
}

/// One line of standard input, without its line ending: the client secret.
fn read_secret() -> Result<Zeroizing<String>, String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprint!("Client secret (it is shown as you type; pipe it in to avoid that): ");
        let _ = std::io::stderr().flush();
    }
    let mut line = Zeroizing::new(String::new());
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("the client secret could not be read: {error}"))?;
    let secret = Zeroizing::new(line.trim_end_matches(['\r', '\n']).to_owned());
    if secret.is_empty() {
        return Err("no client secret was given on standard input".to_string());
    }
    Ok(secret)
}

pub async fn run(pool: PgPool, args: AppsArgs) -> Result<(), String> {
    match args {
        AppsArgs::List => {
            let apps = oauth_apps::entered(&pool).await?;
            if apps.is_empty() {
                println!("No platform OAuth apps are stored.");
            }
            for app in apps {
                println!(
                    "{}\t{}\tclient {}\tscopes: {}\tparameters: {}",
                    app.id, app.integration_id, app.client_id, app.scopes, app.parameters
                );
            }
            Ok(())
        }
        AppsArgs::Remove { id } => {
            if oauth_apps::remove(&pool, &id).await? {
                println!("Removed '{id}'. Connections on it no longer work.");
                Ok(())
            } else {
                Err(format!("there is no platform OAuth app '{id}'"))
            }
        }
        AppsArgs::Put(app) => {
            // Checked before the secret is asked for.
            oauth_apps::check_app(&app)?;
            let secret = read_secret()?;
            let sealed =
                runtara_connections::sealed::from_env(pool.clone(), crate::seal_allowances())?;
            oauth_apps::put(&pool, &sealed, &app, &secret).await?;
            println!("Stored '{}', with its client secret sealed.", app.id);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<AppsArgs, String>> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn other_invocations_fall_through_to_the_server() {
        assert_eq!(parse(&["runtara-server"]), None);
        assert_eq!(parse(&["runtara-server", "seal-connections"]), None);
    }

    #[test]
    fn an_app_is_entered_without_its_secret_among_the_arguments() {
        let put = parse(&[
            "runtara-server",
            COMMAND,
            "put",
            "--id",
            "zoho-eu",
            "--integration=zoho_crm_oauth",
            "--client-id",
            "1000.abc",
            "--scopes",
            "ZohoCRM.modules.ALL",
            "--parameters",
            r#"{"data_center":"eu"}"#,
        ]);
        assert_eq!(
            put,
            Some(Ok(AppsArgs::Put(NewOAuthApp {
                id: "zoho-eu".into(),
                integration_id: "zoho_crm_oauth".into(),
                display_name: "zoho-eu".into(),
                client_id: "1000.abc".into(),
                scopes: "ZohoCRM.modules.ALL".into(),
                required_scopes: String::new(),
                parameters: serde_json::json!({"data_center": "eu"}),
            })))
        );
        for bad in [
            &["runtara-server", COMMAND][..],
            &["runtara-server", COMMAND, "put"][..],
            &["runtara-server", COMMAND, "put", "--id", "a"][..],
            &[
                "runtara-server",
                COMMAND,
                "put",
                "--id=a",
                "--integration=x",
                "--client-id=c",
                "--scopes=s",
                "--client-secret=s",
            ][..],
            &[
                "runtara-server",
                COMMAND,
                "put",
                "--id=a",
                "--integration=x",
                "--client-id=c",
                "--scopes=s",
                "--parameters=[1]",
            ][..],
            &["runtara-server", COMMAND, "put", "--id=a", "--id=b"][..],
            &["runtara-server", COMMAND, "seal"][..],
        ] {
            assert!(matches!(parse(bad), Some(Err(_))), "{bad:?}");
        }
    }

    #[test]
    fn the_other_actions_take_what_they_name() {
        assert_eq!(
            parse(&["runtara-server", COMMAND, "list"]),
            Some(Ok(AppsArgs::List))
        );
        assert_eq!(
            parse(&["runtara-server", COMMAND, "remove", "--id", "hubspot"]),
            Some(Ok(AppsArgs::Remove {
                id: "hubspot".into()
            }))
        );
        for bad in [
            &["runtara-server", COMMAND, "list", "--all"][..],
            &["runtara-server", COMMAND, "remove"][..],
            &["runtara-server", COMMAND, "import", "apps.json"][..],
        ] {
            assert!(matches!(parse(bad), Some(Err(_))), "{bad:?}");
        }
    }
}
