// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::ffi::{OsStr, OsString};

use runtara_component_host::precompile::PRECOMPILE_WORKER_ARGUMENT;
use sqlx::postgres::PgPoolOptions;

fn internal_precompile_component_requested(mut args: impl Iterator<Item = OsString>) -> bool {
    let _program = args.next();
    matches!(args.next().as_deref(), Some(argument) if argument == OsStr::new(PRECOMPILE_WORKER_ARGUMENT))
        && args.next().is_none()
}

fn run_internal_precompile_component() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    if let Err(error) =
        runtara_component_host::precompile::run_precompile_worker(&mut reader, &mut writer)
    {
        eprintln!("internal component precompiler failed: {error:#}");
        return Err(error.into());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if internal_precompile_component_requested(std::env::args_os()) {
        return run_internal_precompile_component();
    }

    dotenvy::dotenv().ok();

    // The server's primary database: workflows, connections, API keys, triggers.
    let database_url = std::env::var("RUNTARA_SERVER_DATABASE_URL").expect(
        "RUNTARA_SERVER_DATABASE_URL is required.\n\
         Set it to your PostgreSQL connection string, e.g.:\n\
         export RUNTARA_SERVER_DATABASE_URL=postgres://runtara:password@localhost/runtara",
    );

    let max_connections: u32 = std::env::var("OBJECT_MODEL_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    println!("Connecting to database...");
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    // Run server-level migrations (workflows, connections, compilations, etc.)
    // ignore_missing(true) allows existing databases that have the old individual
    // smo-runtime migrations in _sqlx_migrations to work without errors.
    let skip_migrations = std::env::var("SKIP_MIGRATIONS")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);

    if skip_migrations {
        println!("Skipping database migrations (SKIP_MIGRATIONS=true)");
    } else {
        println!("Running database migrations...");
        // A failed migration is fatal. Booting anyway produces a server whose
        // code expects tables the database does not have: it answers health
        // checks, serves most requests, and fails only on whatever the missing
        // migration was for -- which is how a checksum mismatch went unnoticed
        // for a week while every migration behind it silently never applied.
        // Deployments that genuinely need to start without migrating have
        // SKIP_MIGRATIONS for that, and say so on purpose.
        if let Err(e) = sqlx::migrate!("./migrations")
            .set_ignore_missing(true)
            .run(&pool)
            .await
        {
            eprintln!("Migration failed: {e}");
            eprintln!(
                "Refusing to start: the schema is not what this build expects. \
                 If a migration was edited after being applied, restore its \
                 original bytes -- an applied migration is immutable, comments \
                 included. Set SKIP_MIGRATIONS=true to start anyway."
            );
            std::process::exit(1);
        }
        println!("Migrations completed");
    }

    runtara_server::start(pool).await
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{PRECOMPILE_WORKER_ARGUMENT, internal_precompile_component_requested};

    #[test]
    fn internal_precompile_mode_requires_the_exact_private_argument() {
        assert!(internal_precompile_component_requested(
            ["runtara-server", PRECOMPILE_WORKER_ARGUMENT]
                .into_iter()
                .map(OsString::from),
        ));
        assert!(!internal_precompile_component_requested(
            ["runtara-server", PRECOMPILE_WORKER_ARGUMENT, "extra"]
                .into_iter()
                .map(OsString::from),
        ));
        assert!(!internal_precompile_component_requested(
            ["runtara-server", "--normal-server-option"]
                .into_iter()
                .map(OsString::from),
        ));
    }
}
