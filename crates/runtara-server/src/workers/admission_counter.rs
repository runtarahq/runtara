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

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

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

/// How often the counter is checked against the durable rows, via
/// `RUNTARA_ADMISSION_RECONCILE_MS` (default 30s).
fn reconcile_interval() -> Duration {
    let ms = std::env::var("RUNTARA_ADMISSION_RECONCILE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms >= 1_000)
        .unwrap_or(30_000);
    Duration::from_millis(ms)
}

/// Read every tenant's counter, pairing it with its durable reservation count.
async fn observed_drift(pool: &PgPool) -> anyhow::Result<HashMap<String, i64>> {
    let Some(manager) = MANAGER.get() else {
        return Ok(HashMap::new());
    };
    let mut conn = manager.clone();

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
    let durable: HashMap<String, i64> = rows.into_iter().collect();

    let mut drift = HashMap::new();
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
        for full_key in batch {
            let Some(tenant_id) = full_key.strip_prefix(KEY_PREFIX) else {
                continue;
            };
            let held: Option<i64> = conn.get(&full_key).await?;
            let held = held.unwrap_or(0);
            let owed = durable.get(tenant_id).copied().unwrap_or(0);
            drift.insert(tenant_id.to_string(), held - owed);
        }
        cursor = next;
        if cursor == 0 {
            break;
        }
    }
    Ok(drift)
}

/// Correct counter drift against the durable rows, without waiting for a restart.
///
/// A process that dies between a reservation's `INCR` and the commit of its
/// durable row leaks that slot, and [`install`] only runs at startup — so
/// before this the leak persisted for the whole life of the process and a
/// long-lived server could drift its way to refusing all work.
///
/// Two properties keep this safe to run against live traffic. Corrections are
/// relative (`DECRBY`/`INCRBY`), never `SET`, so reservations taken while the
/// check runs survive it. And the counter legitimately *leads* the rows —
/// intake increments before its row commits, a release marks its row before
/// decrementing — so only drift present in two consecutive passes is acted on,
/// and only by the amount common to both. No in-flight window outlives that.
async fn reconcile_once(pool: &PgPool, previous: &mut HashMap<String, i64>) -> anyhow::Result<()> {
    let Some(manager) = MANAGER.get() else {
        return Ok(());
    };
    let current = observed_drift(pool).await?;
    let mut conn = manager.clone();

    for (tenant_id, drift) in &current {
        let Some(prior) = previous.get(tenant_id).copied() else {
            continue;
        };
        let settled = settled_drift(prior, *drift);
        if settled == 0 {
            continue;
        }
        let key = key(tenant_id);
        let corrected: i64 = conn.decr(&key, settled).await?;
        warn!(
            tenant_id = %tenant_id,
            settled,
            corrected,
            "Corrected Valkey admission counter drift against the durable reservations"
        );
    }

    *previous = current;
    Ok(())
}

/// The portion of observed drift that survived two consecutive passes.
///
/// The counter legitimately leads the durable rows while intake is in flight,
/// so a single reading proves nothing. Taking only the amount common to both
/// readings — and nothing at all when the sign flips — means a burst of
/// in-flight reservations is never mistaken for a leak, while a genuine leak,
/// which does not shrink on its own, is corrected on the next pass.
fn settled_drift(prior: i64, current: i64) -> i64 {
    if current > 0 && prior > 0 {
        current.min(prior)
    } else if current < 0 && prior < 0 {
        current.max(prior)
    } else {
        0
    }
}

/// Run [`reconcile_once`] on an interval until shutdown.
pub async fn run_reconciler(pool: PgPool, shutdown: crate::shutdown::ShutdownSignal) {
    if !is_enabled() {
        return;
    }
    let interval = reconcile_interval();
    let mut previous: HashMap<String, i64> = HashMap::new();
    info!(
        interval_ms = interval.as_millis() as u64,
        "Valkey admission counter reconciler started"
    );

    loop {
        tokio::time::sleep(interval).await;
        if shutdown.is_shutting_down() {
            info!("Valkey admission counter reconciler stopping on shutdown");
            return;
        }
        if let Err(error) = reconcile_once(&pool, &mut previous).await {
            warn!(error = %error, "Valkey admission counter reconcile pass failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::settled_drift;

    #[test]
    fn transient_in_flight_drift_is_never_corrected() {
        // Intake increments before its durable row commits, so a single pass
        // routinely sees the counter ahead. One observation must never act.
        assert_eq!(settled_drift(0, 3), 0);
        assert_eq!(settled_drift(3, 0), 0);
        // And a burst that has already drained by the second pass is not a leak.
        assert_eq!(settled_drift(-2, 5), 0);
        assert_eq!(settled_drift(5, -2), 0);
    }

    #[test]
    fn only_the_drift_common_to_both_passes_is_corrected() {
        // A leak of 3 with 4 more reservations in flight on the second pass
        // must correct 3, not 7 -- the extra is still legitimately in flight.
        assert_eq!(settled_drift(3, 7), 3);
        assert_eq!(settled_drift(7, 3), 3);
        // Symmetric for a counter that has fallen behind the rows.
        assert_eq!(settled_drift(-3, -7), -3);
        assert_eq!(settled_drift(-7, -3), -3);
    }

    #[test]
    fn a_steady_leak_is_corrected_in_full() {
        assert_eq!(settled_drift(4, 4), 4);
        assert_eq!(settled_drift(-4, -4), -4);
    }
}
