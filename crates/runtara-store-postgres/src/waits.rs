//! Durable instance waits (`instance_waits`, `instance_wait_targets`).
//!
//! Lock order, everywhere: the waiter's `instances` row, then its wait rows
//! in `wait_id` order. Only holders of the waiter's row lock a wait row. The
//! trigger a finishing target fires (migration 040) reads wait rows without
//! locking them, takes the waiter's row only with `SKIP LOCKED`, and nudges
//! `instance_wait_targets`; everyone else touches target rows with
//! `SKIP LOCKED` only, so a finishing run never waits on a waiter and no
//! cycle can form.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use runtara_core::persistence::ExternalOutcomeKind;
use runtara_core::persistence::waits::*;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::PostgresPersistence;

fn storage(error: impl std::fmt::Display) -> WaitError {
    WaitError::Storage(error.to_string())
}

#[derive(sqlx::FromRow)]
struct WaitRow {
    waiter_instance_id: String,
    wait_id: String,
    generation: Uuid,
    tenant_id: String,
    mode: String,
    targets: Vec<String>,
    fingerprint: String,
    deadline: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    state: String,
    resolution: Option<String>,
    finished: Option<Vec<String>>,
    resolved_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
}

const WAIT_COLUMNS: &str = "waiter_instance_id, wait_id, generation, tenant_id, mode, targets, \
     fingerprint, deadline, created_at, state, resolution, finished, resolved_at, closed_at";

impl WaitRow {
    fn record(self) -> WaitResult<(Uuid, WaitRecord)> {
        let incomplete = || storage("incomplete instance wait record");
        let state = match self.state.as_str() {
            "pending" => WaitState::Pending,
            "resolved" => WaitState::Resolved {
                resolution: self
                    .resolution
                    .as_deref()
                    .and_then(WaitResolution::parse)
                    .ok_or_else(incomplete)?,
                resolved_at: self.resolved_at.ok_or_else(incomplete)?,
                finished: self.finished.ok_or_else(incomplete)?,
            },
            "closed" => WaitState::Closed {
                closed_at: self.closed_at.ok_or_else(incomplete)?,
            },
            _ => return Err(storage("unknown instance wait state")),
        };
        Ok((
            self.generation,
            WaitRecord {
                waiter_instance_id: self.waiter_instance_id,
                wait_id: self.wait_id,
                tenant_id: self.tenant_id,
                mode: WaitMode::parse(&self.mode).ok_or_else(incomplete)?,
                targets: self.targets,
                fingerprint: self.fingerprint,
                deadline: self.deadline,
                created_at: self.created_at,
                state,
            },
        ))
    }
}

/// Lock the waiter's row (first in the lock order) and return its status.
async fn lock_waiter(db: &mut PgConnection, tenant: &str, waiter: &str) -> WaitResult<String> {
    sqlx::query_scalar(
        "SELECT status::text FROM instances WHERE instance_id = $1 AND tenant_id = $2 FOR UPDATE",
    )
    .bind(waiter)
    .bind(tenant)
    .fetch_optional(db)
    .await
    .map_err(storage)?
    .ok_or(WaitError::NotFound)
}

/// Lock and read one wait of a waiter whose row the caller holds.
async fn load(
    db: &mut PgConnection,
    waiter: &str,
    wait_id: &str,
) -> WaitResult<Option<(Uuid, WaitRecord)>> {
    sqlx::query_as::<_, WaitRow>(&format!(
        "SELECT {WAIT_COLUMNS} FROM instance_waits \
         WHERE waiter_instance_id = $1 AND wait_id = $2 FOR UPDATE"
    ))
    .bind(waiter)
    .bind(wait_id)
    .fetch_optional(db)
    .await
    .map_err(storage)?
    .map(WaitRow::record)
    .transpose()
}

async fn clock(db: &mut PgConnection) -> WaitResult<DateTime<Utc>> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(db)
        .await
        .map_err(storage)
}

/// What the rows say of each target, in `targets` order. An instance row of
/// the tenant wins over a published outcome.
pub(crate) async fn target_states(
    db: &mut PgConnection,
    tenant: &str,
    targets: &[String],
) -> WaitResult<Vec<WaitTarget>> {
    type Row = (
        String,
        Option<String>,
        Option<DateTime<Utc>>,
        Option<String>,
        Option<DateTime<Utc>>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.id, i.status::text, i.finished_at, o.outcome, o.published_at \
         FROM unnest($1::text[]) WITH ORDINALITY AS t(id, n) \
         LEFT JOIN instances AS i ON i.instance_id = t.id AND i.tenant_id = $2 \
         LEFT JOIN instance_external_outcomes AS o ON o.instance_id = t.id AND o.tenant_id = $2 \
         ORDER BY t.n",
    )
    .bind(targets)
    .bind(tenant)
    .fetch_all(db)
    .await
    .map_err(storage)?;
    rows.into_iter()
        .map(|(id, status, finished_at, outcome, published_at)| {
            let state = match (status, outcome, published_at) {
                (Some(status), _, _) => TargetState::Instance {
                    status: crate::encoding::status_from_str(&status).map_err(storage)?,
                    finished_at,
                },
                (None, Some(outcome), Some(published_at)) => TargetState::Outcome {
                    outcome: ExternalOutcomeKind::parse(&outcome)
                        .ok_or_else(|| storage("unknown external outcome"))?,
                    published_at,
                },
                _ => TargetState::Unknown,
            };
            Ok(WaitTarget {
                instance_id: id,
                state,
            })
        })
        .collect()
}

/// Remove the target rows of a wait that is no longer pending, skipping any a
/// finishing run is nudging right now; the reconciler prunes those later.
async fn prune_targets(
    db: &mut PgConnection,
    waiter: &str,
    wait_id: &str,
    generation: Uuid,
) -> WaitResult<()> {
    sqlx::query(
        "DELETE FROM instance_wait_targets \
         WHERE (waiter_instance_id, wait_id, generation, target_instance_id) IN ( \
             SELECT waiter_instance_id, wait_id, generation, target_instance_id \
             FROM instance_wait_targets \
             WHERE waiter_instance_id = $1 AND wait_id = $2 AND generation = $3 \
             FOR UPDATE SKIP LOCKED)",
    )
    .bind(waiter)
    .bind(wait_id)
    .bind(generation)
    .execute(db)
    .await
    .map_err(storage)?;
    Ok(())
}

/// Under the waiter's and the wait's locks: apply the rule to a pending wait
/// and persist its resolution the first time it resolves. Returns the target
/// states it read.
async fn settle(
    db: &mut PgConnection,
    generation: Uuid,
    record: &mut WaitRecord,
) -> WaitResult<Vec<WaitTarget>> {
    let states = target_states(db, &record.tenant_id, &record.targets).await?;
    if record.state != WaitState::Pending {
        return Ok(states);
    }
    let now = clock(db).await?;
    if let Some((resolution, finished)) = record.evaluate(&states, now) {
        sqlx::query(
            "UPDATE instance_waits SET state = 'resolved', resolution = $3, finished = $4, \
             resolved_at = $5 WHERE waiter_instance_id = $1 AND wait_id = $2 AND state = 'pending'",
        )
        .bind(&record.waiter_instance_id)
        .bind(&record.wait_id)
        .bind(resolution.as_str())
        .bind(&finished)
        .bind(now)
        .execute(&mut *db)
        .await
        .map_err(storage)?;
        prune_targets(db, &record.waiter_instance_id, &record.wait_id, generation).await?;
        record.state = WaitState::Resolved {
            resolution,
            resolved_at: now,
            finished,
        };
    }
    Ok(states)
}

/// Schedule the wake of a waiter parked on `wait_id`, once per park. The
/// caller holds the waiter's row. A paused waiter (no suspension reason, no
/// park) is never stamped.
pub(crate) async fn stamp(db: &mut PgConnection, waiter: &str, wait_id: &str) -> WaitResult<bool> {
    let parked: Option<bool> = sqlx::query_scalar(
        "UPDATE instance_input_parks AS p SET wake_scheduled = TRUE \
         FROM instances AS i \
         WHERE p.instance_id = $1 AND i.instance_id = $1 \
           AND i.status = 'suspended' AND i.termination_reason = 'waiting_instances' \
           AND NOT p.wake_scheduled AND $2 = ANY (p.wait_ids) \
         RETURNING TRUE",
    )
    .bind(waiter)
    .bind(wait_id)
    .fetch_optional(&mut *db)
    .await
    .map_err(storage)?;
    if parked.is_none() {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE instances SET sleep_until = LEAST(sleep_until, clock_timestamp()), \
         wake_reason = 'instances_terminal' WHERE instance_id = $1",
    )
    .bind(waiter)
    .execute(db)
    .await
    .map_err(storage)?;
    Ok(true)
}

/// A park's own check, under its lock of the waiter: evaluate every wait it
/// parks on and stamp its wake when one is already resolved, closed or
/// missing.
pub(crate) async fn park_on(
    db: &mut PgConnection,
    waiter: &str,
    wait_ids: &[String],
) -> WaitResult<()> {
    let mut ids = wait_ids.to_vec();
    ids.sort();
    ids.dedup();
    for wait_id in ids {
        let wake = match load(db, waiter, &wait_id).await? {
            Some((generation, mut record)) if record.state == WaitState::Pending => {
                settle(db, generation, &mut record).await?;
                record.state != WaitState::Pending
            }
            _ => true,
        };
        if wake {
            stamp(db, waiter, &wait_id).await?;
        }
    }
    Ok(())
}

/// Insert a fresh registration of `spec` and its target rows.
async fn insert(
    db: &mut PgConnection,
    tenant: &str,
    waiter: &str,
    wait_id: &str,
    spec: &WaitSpec,
) -> WaitResult<(Uuid, WaitRecord)> {
    let (generation, record) = sqlx::query_as::<_, WaitRow>(&format!(
        "INSERT INTO instance_waits \
             (waiter_instance_id, wait_id, tenant_id, mode, targets, fingerprint, deadline, state) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending') RETURNING {WAIT_COLUMNS}"
    ))
    .bind(waiter)
    .bind(wait_id)
    .bind(tenant)
    .bind(spec.mode().as_str())
    .bind(spec.targets())
    .bind(spec.fingerprint())
    .bind(spec.deadline())
    .fetch_one(&mut *db)
    .await
    .map_err(storage)?
    .record()?;
    sqlx::query(
        "INSERT INTO instance_wait_targets \
             (waiter_instance_id, wait_id, generation, target_instance_id) \
         SELECT $1, $2, $3, unnest($4::text[])",
    )
    .bind(waiter)
    .bind(wait_id)
    .bind(generation)
    .bind(spec.targets())
    .execute(db)
    .await
    .map_err(storage)?;
    Ok((generation, record))
}

impl PostgresPersistence {
    /// One waiter's reconciliation, in its own transaction: take its row if
    /// nobody holds it, then settle `waits` (or, with `None`, the waits its
    /// nudges name, claiming them first) and stamp its wake. Returns whether
    /// it was woken.
    async fn reconcile_waiter(&self, waiter: &str, waits: Option<&str>) -> WaitResult<bool> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let locked: Option<String> = sqlx::query_scalar(
            "SELECT instance_id FROM instances WHERE instance_id = $1 FOR UPDATE SKIP LOCKED",
        )
        .bind(waiter)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        if locked.is_none() {
            // Held (or gone): its nudges stay for the next pass.
            return Ok(false);
        }
        let mut ids: Vec<String> = match waits {
            Some(wait_id) => vec![wait_id.to_owned()],
            // Claim the nudges before reading any target: a finish that
            // commits after this statement leaves a nudge of its own.
            None => sqlx::query_scalar(
                "UPDATE instance_wait_targets SET wake_pending = FALSE \
                 WHERE (waiter_instance_id, wait_id, generation, target_instance_id) IN ( \
                     SELECT waiter_instance_id, wait_id, generation, target_instance_id \
                     FROM instance_wait_targets WHERE waiter_instance_id = $1 AND wake_pending \
                     FOR UPDATE SKIP LOCKED) \
                 RETURNING wait_id",
            )
            .bind(waiter)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?,
        };
        ids.sort();
        ids.dedup();
        let mut woken = false;
        for wait_id in ids {
            let Some((generation, mut record)) = load(&mut tx, waiter, &wait_id).await? else {
                continue;
            };
            match record.state {
                WaitState::Pending => {
                    settle(&mut tx, generation, &mut record).await?;
                }
                _ => prune_targets(&mut tx, waiter, &wait_id, generation).await?,
            }
            if waits.is_some() {
                sqlx::query(
                    "UPDATE instance_waits SET last_reconciled_at = clock_timestamp() \
                     WHERE waiter_instance_id = $1 AND wait_id = $2",
                )
                .bind(waiter)
                .bind(&wait_id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
            if record.state != WaitState::Pending {
                woken |= stamp(&mut tx, waiter, &wait_id).await?;
            }
        }
        tx.commit().await.map_err(storage)?;
        Ok(woken)
    }
}

#[async_trait]
impl InstanceWaits for PostgresPersistence {
    async fn register_or_evaluate(
        &self,
        tenant: &str,
        waiter: &str,
        wait_id: &str,
        spec: &WaitSpec,
    ) -> WaitResult<WaitView> {
        spec.validate(waiter, wait_id)?;
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let status = lock_waiter(&mut tx, tenant, waiter).await?;
        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(WaitError::Inactive);
        }
        let (generation, mut record) = match load(&mut tx, waiter, wait_id).await? {
            Some((_, record)) if matches!(record.state, WaitState::Closed { .. }) => {
                // A retried operation waits again: replace the closed wait.
                // Its target rows carry the old generation and are pruned.
                sqlx::query(
                    "DELETE FROM instance_waits WHERE waiter_instance_id = $1 AND wait_id = $2",
                )
                .bind(waiter)
                .bind(wait_id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
                insert(&mut tx, tenant, waiter, wait_id, spec).await?
            }
            Some((generation, record)) => {
                if record.fingerprint != spec.fingerprint() {
                    return Err(WaitError::Conflict);
                }
                (generation, record)
            }
            None => insert(&mut tx, tenant, waiter, wait_id, spec).await?,
        };
        let states = settle(&mut tx, generation, &mut record).await?;
        tx.commit().await.map_err(storage)?;
        WaitView::assemble(record, states)
    }

    async fn poll_wait(&self, tenant: &str, waiter: &str, wait_id: &str) -> WaitResult<WaitView> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        lock_waiter(&mut tx, tenant, waiter).await?;
        let (generation, mut record) = load(&mut tx, waiter, wait_id)
            .await?
            .ok_or(WaitError::NotFound)?;
        if matches!(record.state, WaitState::Closed { .. }) {
            return Err(WaitError::Closed);
        }
        let states = settle(&mut tx, generation, &mut record).await?;
        tx.commit().await.map_err(storage)?;
        WaitView::assemble(record, states)
    }

    async fn close_wait(&self, tenant: &str, waiter: &str, wait_id: &str) -> WaitResult<bool> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        lock_waiter(&mut tx, tenant, waiter).await?;
        let Some((generation, record)) = load(&mut tx, waiter, wait_id).await? else {
            return Ok(false);
        };
        if matches!(record.state, WaitState::Closed { .. }) {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE instance_waits SET state = 'closed', closed_at = clock_timestamp() \
             WHERE waiter_instance_id = $1 AND wait_id = $2",
        )
        .bind(waiter)
        .bind(wait_id)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        prune_targets(&mut tx, waiter, wait_id, generation).await?;
        tx.commit().await.map_err(storage)?;
        Ok(true)
    }

    async fn delete_resolved_wait(
        &self,
        tenant: &str,
        waiter: &str,
        wait_id: &str,
    ) -> WaitResult<bool> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        lock_waiter(&mut tx, tenant, waiter).await?;
        let Some((generation, record)) = load(&mut tx, waiter, wait_id).await? else {
            return Ok(false);
        };
        if record.state == WaitState::Pending {
            return Ok(false);
        }
        sqlx::query("DELETE FROM instance_waits WHERE waiter_instance_id = $1 AND wait_id = $2")
            .bind(waiter)
            .bind(wait_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        prune_targets(&mut tx, waiter, wait_id, generation).await?;
        tx.commit().await.map_err(storage)?;
        Ok(true)
    }

    async fn reconcile_wait_wakes(&self, limit: u32) -> WaitResult<u64> {
        let full = self
            .wait_polls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(FULL_RECONCILE_EVERY);
        let limit = i64::from(limit);
        let mut woken = std::collections::BTreeSet::new();
        // Pass A: waiters with nudges, read without locks.
        let nudged: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT waiter_instance_id FROM instance_wait_targets \
             WHERE wake_pending ORDER BY waiter_instance_id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        for waiter in nudged {
            if self.reconcile_waiter(&waiter, None).await? {
                woken.insert(waiter);
            }
        }
        if full {
            // Pass B: pending waits of parked waiters, least recently
            // reconciled first, so the cursor rotates through all of them.
            let parked: Vec<(String, String)> = sqlx::query_as(
                "SELECT w.waiter_instance_id, w.wait_id FROM instance_waits AS w \
                 JOIN instance_input_parks AS p ON p.instance_id = w.waiter_instance_id \
                  AND w.wait_id = ANY (p.wait_ids) AND NOT p.wake_scheduled \
                 WHERE w.state = 'pending' \
                 ORDER BY w.last_reconciled_at NULLS FIRST, w.waiter_instance_id, w.wait_id \
                 LIMIT $1",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
            for (waiter, wait_id) in parked {
                if self.reconcile_waiter(&waiter, Some(&wait_id)).await? {
                    woken.insert(waiter);
                }
            }
            // Target rows of waits that are gone or no longer pending.
            sqlx::query(
                "DELETE FROM instance_wait_targets \
                 WHERE (waiter_instance_id, wait_id, generation, target_instance_id) IN ( \
                     SELECT t.waiter_instance_id, t.wait_id, t.generation, t.target_instance_id \
                     FROM instance_wait_targets AS t \
                     LEFT JOIN instance_waits AS w \
                       ON w.waiter_instance_id = t.waiter_instance_id \
                      AND w.wait_id = t.wait_id AND w.generation = t.generation \
                     WHERE w.state IS DISTINCT FROM 'pending' \
                     LIMIT $1 FOR UPDATE OF t SKIP LOCKED)",
            )
            .bind(limit)
            .execute(&self.pool)
            .await
            .map_err(storage)?;
        }
        Ok(woken.len() as u64)
    }
}
