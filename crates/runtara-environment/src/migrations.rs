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
//! in the correct order. The migrations are merged into a single migrator
//! so SQLx sees them as one unified set.
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

use sqlx::migrate::{MigrateError, Migration, Migrator};
use std::borrow::Cow;

/// Environment-specific migrations embedded at compile time.
///
/// These use version numbers starting at 20250101000000 to ensure
/// they sort after core migrations (001, 002, ...).
static ENV_MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Combined migrator with both core and environment migrations.
///
/// This struct implements a custom migration source that merges
/// runtara-core's PostgreSQL migrations with environment-specific migrations.
#[derive(Debug)]
struct CombinedMigrations;

impl<'s> sqlx::migrate::MigrationSource<'s> for CombinedMigrations {
    fn resolve(
        self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<Migration>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send
                + 's,
        >,
    > {
        Box::pin(async move {
            // Get core migrations
            let core_migrations: Vec<Migration> =
                runtara_core::migrations::POSTGRES.iter().cloned().collect();

            // Get environment migrations
            let env_migrations: Vec<Migration> = ENV_MIGRATOR.iter().cloned().collect();

            // Combine and sort by version
            let mut all_migrations = core_migrations;
            all_migrations.extend(env_migrations);
            all_migrations.sort_by_key(|m| m.version);

            Ok(all_migrations)
        })
    }
}

/// PostgreSQL migrator with all migrations (core + environment).
///
/// This is created lazily on first use since we need to merge migrations
/// from two sources at runtime.
pub async fn migrator() -> Result<Migrator, MigrateError> {
    Migrator::new(CombinedMigrations).await
}

/// Run all migrations (core + environment).
///
/// This function creates a combined migrator with both runtara-core
/// and environment migrations, then runs them as a single unified set.
///
/// Safe to call multiple times; already-applied migrations are skipped.
pub async fn run(pool: &sqlx::PgPool) -> Result<(), MigrateError> {
    let migrator = migrator().await?;
    migrator.run(pool).await
}

/// Get an iterator over all migrations (core + environment).
///
/// Returns migrations sorted by version number.
pub fn iter() -> impl Iterator<Item = Cow<'static, Migration>> {
    let core_iter = runtara_core::migrations::POSTGRES.iter().map(Cow::Borrowed);
    let env_iter = ENV_MIGRATOR.iter().map(Cow::Borrowed);

    let mut all: Vec<_> = core_iter.chain(env_iter).collect();
    all.sort_by_key(|m| m.version);
    all.into_iter()
}
