// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database migrations for runtara-environment.
//!
//! Environment extends runtara-core's schema with additional tables for:
//! - Image registry
//! - Container lifecycle tracking
//! - Instance-image associations
//!
//! Calling [`run`] will apply both runtara-core and environment migrations
//! in the correct order.
//!
//! # Example
//!
//! ```ignore
//! use sqlx::PgPool;
//! use runtara_environment::migrations;
//!
//! let pool = PgPool::connect(&database_url).await?;
//! migrations::run(&pool).await?;
//! ```

use sqlx::migrate::MigrateError;

/// Environment migrator with environment-specific migrations embedded.
///
/// Note: Environment migrations use version numbers starting at 100 to ensure
/// they always sort after core migrations in the `_sqlx_migrations` table.
static ENV_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Run all migrations (core + environment).
///
/// This function:
/// 1. Runs all runtara-core migrations first
/// 2. Runs environment-specific migrations
///
/// Safe to call multiple times; already-applied migrations are skipped.
pub async fn run(pool: &sqlx::PgPool) -> Result<(), MigrateError> {
    // First: ensure core schema exists
    runtara_core::migrations::run_postgres(pool).await?;

    // Then: apply environment extensions
    ENV_MIGRATOR.run(pool).await
}
