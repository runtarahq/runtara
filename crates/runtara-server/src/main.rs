// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let database_url = std::env::var("OBJECT_MODEL_DATABASE_URL").expect(
        "OBJECT_MODEL_DATABASE_URL environment variable is required.\n\
         Set it to your PostgreSQL connection string, e.g.:\n\
         export OBJECT_MODEL_DATABASE_URL=postgres://runtara:password@localhost/runtara_objects",
    );

    let max_connections: u32 = std::env::var("OBJECT_MODEL_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(&database_url)
        .await
        .expect("Failed to connect to object model database");

    runtara_server::start(pool).await
}
