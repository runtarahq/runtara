// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Valkey-backed source admission counting.
//!
//! Admission is a counter with a bound, read and written once per intake and
//! once per terminal release. Keeping that counter in a PostgreSQL row makes
//! every one of those touch the *same* row and hold its lock until the
//! surrounding transaction commits, so the whole pipeline serializes on it:
//! measured on the benchmark VM at 600/s offered, the increment alone averaged
//! 104 ms of lock wait against 0.08-0.18 ms for every other query in the
//! pipeline.
//!
//! `INCR`/`DECR` are atomic in Valkey without holding anything across a
//! transaction, which is what the bound actually needs.
//!
//! Durability: the authoritative durable record stays in PostgreSQL
//! (`execution_admission_reservations`), so a lost or restarted Valkey costs
//! only the cached count, which [`install`] rebuilds from those rows. The
//! counter is a fast bound over durable truth, not the truth itself.

use std::sync::OnceLock;

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use tracing::{info, warn};

static MANAGER: OnceLock<ConnectionManager> = OnceLock::new();

/// Whether Valkey-backed admission counting is active for this process.
pub fn is_enabled() -> bool {
    MANAGER.get().is_some()
}

const KEY_PREFIX: &str = "runtara:admission:reserved:";

fn key(tenant_id: &str) -> String {
    format!("{KEY_PREFIX}{tenant_id}")
}

/// Adopt `manager` for admission counting and seed every tenant's counter from
/// the durable reservation rows.
///
/// Seeding is what makes the counter safe to lose: an empty or stale Valkey
/// starts over from the same rows the release path maintains. Called once at
/// startup, before any intake can run.
pub async fn install(manager: ConnectionManager, pool: &PgPool) -> anyhow::Result<()> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT tenant_id, count(*)
        FROM execution_admission_reservations
        WHERE released_at IS NULL
        GROUP BY tenant_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut conn = manager.clone();

    // Clear first, then write what the rows say. Writing only the tenants that
    // still hold reservations would leave a stale counter in place for every
    // tenant that has since drained -- and a counter stuck above the limit
    // refuses that tenant's work indefinitely, with nothing to bring it back
    // down. Dropping the whole namespace makes the seed reflect the durable
    // rows exactly, including the tenants whose correct value is zero.
    let mut cleared = 0usize;
    let mut cursor: u64 = 0;
    loop {
        let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(format!("{KEY_PREFIX}*"))
            .arg("COUNT")
            .arg(500)
            .query_async(&mut conn)
            .await?;
        if !batch.is_empty() {
            cleared += batch.len();
            let _: () = conn.del(batch).await?;
        }
        cursor = next;
        if cursor == 0 {
            break;
        }
    }

    for (tenant_id, held) in &rows {
        let _: () = conn.set(key(tenant_id), *held).await?;
    }
    info!(
        tenants = rows.len(),
        cleared, "Valkey admission counter seeded from durable reservations"
    );

    if MANAGER.set(manager).is_err() {
        warn!("Valkey admission counter was already installed; keeping the first manager");
    }
    Ok(())
}

/// Take one reservation if the tenant is under `limit`.
///
/// `Ok(false)` means the bound is already met. The over-limit increment is
/// given straight back, so a rejected intake leaves the counter unchanged.
pub async fn try_reserve(tenant_id: &str, limit: i64) -> Result<bool, redis::RedisError> {
    let Some(manager) = MANAGER.get() else {
        return Ok(true);
    };
    let mut conn = manager.clone();
    let held: i64 = conn.incr(key(tenant_id), 1).await?;
    if held > limit {
        let _: i64 = conn.decr(key(tenant_id), 1).await?;
        return Ok(false);
    }
    Ok(true)
}

/// Give one reservation back.
///
/// Clamped at zero so a double release — or a release of work reserved before
/// a restart reseeded the counter — cannot drive it negative and hand out more
/// admission than the bound allows.
pub async fn release(tenant_id: &str) -> Result<(), redis::RedisError> {
    let Some(manager) = MANAGER.get() else {
        return Ok(());
    };
    let mut conn = manager.clone();
    let held: i64 = conn.decr(key(tenant_id), 1).await?;
    if held < 0 {
        let _: () = conn.set(key(tenant_id), 0).await?;
    }
    Ok(())
}
