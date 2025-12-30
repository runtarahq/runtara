// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database migrations for runtara-core.
//!
//! This module exposes embedded migrations that can be run programmatically.
//! Products embedding runtara-core can call these functions to set up the database schema.
//!
//! # Example
//!
//! ```ignore
//! use sqlx::PgPool;
//! use runtara_core::migrations;
//!
//! let pool = PgPool::connect(&database_url).await?;
//! migrations::run_postgres(&pool).await?;
//! ```

use sqlx::migrate::MigrateError;

/// PostgreSQL migrator with all core migrations embedded.
pub static POSTGRES: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/postgresql");

/// SQLite migrator with all core migrations embedded.
pub static SQLITE: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/sqlite");

/// Run PostgreSQL migrations.
///
/// Applies all pending migrations to the database. Safe to call multiple times;
/// already-applied migrations are skipped.
pub async fn run_postgres(pool: &sqlx::PgPool) -> Result<(), MigrateError> {
    POSTGRES.run(pool).await
}

/// Run SQLite migrations.
///
/// Applies all pending migrations to the database. Safe to call multiple times;
/// already-applied migrations are skipped.
pub async fn run_sqlite(pool: &sqlx::SqlitePool) -> Result<(), MigrateError> {
    SQLITE.run(pool).await
}
