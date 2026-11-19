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
/// The tenants whose counters this process owns. One Valkey may hold other
/// processes' tenants; their counters are never read or corrected here.
static TENANTS: OnceLock<Vec<String>> = OnceLock::new();
/// The tenants whose counters [`install`] seeded as their keeper.
static SEEDED_AS_KEEPER: OnceLock<Vec<String>> = OnceLock::new();

/// Whether Valkey-backed admission counting is active for this process.
pub fn is_enabled() -> bool {
    MANAGER.get().is_some()
}

fn key(tenant_id: &str) -> String {
    format!("{tenant_id}:admission:reserved")
}

/// The key of earlier versions, removed when the counter is seeded.
fn legacy_key(tenant_id: &str) -> String {
    format!("runtara:admission:reserved:{tenant_id}")
}

/// The lease of a tenant's counter's keeper: the one engine that seeds it
/// over and corrects it.
fn keeper_lease(tenant_id: &str) -> String {
    format!("admission-counter:{tenant_id}")
}

/// How long a keeper's lease stands unrenewed.
const KEEPER_LEASE: Duration = Duration::from_secs(15);

fn tenants() -> &'static [String] {
    TENANTS.get().map(Vec::as_slice).unwrap_or_default()
}

/// Counts admissions in Valkey from now on, without seeding the counters:
/// for a process that only adds to them (intake). The engine seeds them from
/// its reservations ([`install`]) and reconciles them; a process that reset
/// them from a query across tenants would need to be the platform, and
/// intake is not.
pub fn install_without_seeding(manager: ConnectionManager) {
    let _ = TENANTS.set(crate::config::served_tenants().to_vec());
    let _ = MANAGER.set(manager);
}

/// Adopt `manager` for admission counting and seed each of `tenants`' counters
/// from the durable reservation rows.
///
/// Seeding is what makes the counter safe to lose: an empty or stale Valkey
/// starts over from the same rows the release path maintains. Called once at
/// startup, before any intake can run.
pub async fn install(
    manager: ConnectionManager,
    pool: &PgPool,
    tenants: Vec<String>,
) -> anyhow::Result<()> {
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
    let mut conn = manager.clone();

    // Write every served tenant's count, zero included: seeding only the
    // tenants that still hold reservations would leave a stale counter in
    // place for one that has since drained -- and a counter stuck above the
    // limit refuses that tenant's work indefinitely.
    //
    // Only the tenant's keeper writes it over (docs/multiple-engines-plan.md,
    // E2): an engine that starts beside a live keeper would lose the
    // keeper's reservations in flight, so it seeds only a counter that is
    // not there.
    let mut kept = Vec::new();
    for tenant_id in &tenants {
        let held = durable.get(tenant_id).copied().unwrap_or(0);
        let keeper = crate::peers::singleton::take_or_renew(
            &manager,
            &keeper_lease(tenant_id),
            crate::peers::engine_id(),
            KEEPER_LEASE,
        )
        .await?;
        if keeper {
            let _: () = conn.set(key(tenant_id), held).await?;
            kept.push(tenant_id.clone());
        } else {
            let _: bool = conn.set_nx(key(tenant_id), held).await?;
        }
        if !crate::config::valkey_tenant_scoped() {
            let _: () = conn.del(legacy_key(tenant_id)).await?;
        }
    }
    info!(
        tenants = tenants.len(),
        kept = kept.len(),
        "Valkey admission counter seeded from durable reservations"
    );
    let _ = SEEDED_AS_KEEPER.set(kept);
    let _ = TENANTS.set(tenants);

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

/// Read each served tenant's counter, pairing it with its durable reservation count.
async fn observed_drift(pool: &PgPool, kept: &[String]) -> anyhow::Result<HashMap<String, i64>> {
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
    for tenant_id in kept {
        let held: Option<i64> = conn.get(key(tenant_id)).await?;
        let owed = durable.get(tenant_id).copied().unwrap_or(0);
        drift.insert(tenant_id.clone(), held.unwrap_or(0) - owed);
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
async fn reconcile_once(
    pool: &PgPool,
    kept: &[String],
    previous: &mut HashMap<String, i64>,
) -> anyhow::Result<()> {
    let Some(manager) = MANAGER.get() else {
        return Ok(());
    };
    let current = observed_drift(pool, kept).await?;
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

/// Seeds `tenant_id`'s counter over from the durable reservations: done by
/// an engine that has just become its keeper.
async fn reseed(pool: &PgPool, tenant_id: &str) -> anyhow::Result<()> {
    let Some(manager) = MANAGER.get() else {
        return Ok(());
    };
    let held: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM execution_admission_reservations \
         WHERE released_at IS NULL AND tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await?;
    let mut conn = manager.clone();
    let _: () = conn.set(key(tenant_id), held).await?;
    info!(tenant_id = %tenant_id, held, "this engine keeps the tenant's admission counter now; reseeded it");
    Ok(())
}

/// Which tenants' counters this engine keeps now, of `keepers`; reseeds a
/// counter whose keeper it has just become, and forgets the drift of one
/// it no longer keeps.
async fn kept_now(
    pool: &PgPool,
    keepers: &[(String, crate::peers::singleton::Singleton)],
    kept_before: &mut std::collections::HashSet<String>,
    previous: &mut HashMap<String, i64>,
) -> Vec<String> {
    let mut kept = Vec::new();
    for (tenant_id, lease) in keepers {
        if lease.is_held() {
            if !kept_before.contains(tenant_id) {
                if let Err(error) = reseed(pool, tenant_id).await {
                    warn!(tenant_id = %tenant_id, error = %error, "could not reseed the admission counter");
                    continue;
                }
                kept_before.insert(tenant_id.clone());
            }
            kept.push(tenant_id.clone());
        } else {
            kept_before.remove(tenant_id);
            previous.remove(tenant_id);
        }
    }
    kept
}

/// Run [`reconcile_once`] on an interval until shutdown, for the tenants
/// whose counters this engine keeps (one engine per tenant: E2 of
/// docs/multiple-engines-plan.md).
pub async fn run_reconciler(pool: PgPool, shutdown: crate::shutdown::ShutdownSignal) {
    let Some(manager) = MANAGER.get().cloned() else {
        return;
    };
    let interval = reconcile_interval();
    let mut previous: HashMap<String, i64> = HashMap::new();
    let keepers: Vec<(String, crate::peers::singleton::Singleton)> = tenants()
        .iter()
        .map(|tenant_id| {
            (
                tenant_id.clone(),
                crate::peers::singleton::Singleton::start(
                    manager.clone(),
                    keeper_lease(tenant_id),
                    crate::peers::engine_id(),
                    KEEPER_LEASE,
                    shutdown.clone(),
                ),
            )
        })
        .collect();
    // What `install` seeded as the keeper needs no reseeding when the lease
    // it took is first seen held.
    let mut kept_before: std::collections::HashSet<String> = SEEDED_AS_KEEPER
        .get()
        .map(|kept| kept.iter().cloned().collect())
        .unwrap_or_default();
    info!(
        interval_ms = interval.as_millis() as u64,
        kept = kept_before.len(),
        "Valkey admission counter reconciler started"
    );

    loop {
        // Wake on the shutdown signal rather than only at the end of the
        // sleep. The interval is 30 s by default and the intake grace is 5 s,
        // so a worker that noticed shutdown only on the next tick could never
        // stop inside the grace — it would be reported as a straggler and
        // aborted on every single shutdown.
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.clone().wait() => {}
        }
        if shutdown.is_shutting_down() {
            info!("Valkey admission counter reconciler stopping on shutdown");
            return;
        }
        let kept = kept_now(&pool, &keepers, &mut kept_before, &mut previous).await;
        if let Err(error) = reconcile_once(&pool, &kept, &mut previous).await {
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
