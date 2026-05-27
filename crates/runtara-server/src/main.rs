// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        match sqlx::migrate!("./migrations")
            .set_ignore_missing(true)
            .run(&pool)
            .await
        {
            Ok(_) => println!("Migrations completed"),
            Err(e) => {
                eprintln!("Warning: Migration failed: {}", e);
                println!("Continuing without migrations...");
            }
        }
    }

    runtara_server::start(pool).await
}
