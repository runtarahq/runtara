// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Notices that a run has something new, for whoever follows one.
//!
//! When a run leaves an event, or ends, this store says so on a Postgres
//! channel, in the transaction that made it true: the notice arrives when
//! what it is about can be read, and is not sent if that is rolled back.
//! What follows a run (a conversation it carries) listens here and reads
//! the run when told, and not on a clock.
//!
//! A notice may be missed: a listener that was reconnecting hears nothing
//! of what was said meanwhile. Whoever listens also looks now and then.

use sqlx::PgPool;
use sqlx::postgres::PgListener;

/// The channel. A notice's payload is the run's instance id.
pub const RUN: &str = "runtara_run";

/// Listens for the runs of this database that have something new.
pub async fn listen(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(RUN).await?;
    Ok(listener)
}

/// Says that `instance` has something new, in `executor`'s transaction.
pub(crate) async fn tell<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    instance: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(RUN)
        .bind(instance)
        .execute(executor)
        .await
        .map(|_| ())
}
