//! Durable admission and delivery for asynchronous workflow executions.
//!
//! A source request is accepted only after its idempotency record, source
//! admission reservation, and outbox record commit together.  The relay is
//! deliberately at-least-once: a process may die after `XADD` and before the
//! delivery mark, so consumers continue to deduplicate on `instance_id` while
//! the outbox retries the same request identity.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use thiserror::Error;
use tokio::time::MissedTickBehavior;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::runtime_client::{InstanceStatus, RuntimeClient};
use crate::shutdown::ShutdownSignal;

const DEFAULT_OUTBOX_DEADLINE: Duration = Duration::from_secs(5 * 60);
const DEFAULT_OUTBOX_LEASE: Duration = Duration::from_secs(30);
const DEFAULT_OUTBOX_RETRY_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_OUTBOX_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_OUTBOX_BATCH_SIZE: usize = 50;
const DEFAULT_ADMISSION_RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_ADMISSION_RECONCILE_BATCH_SIZE: usize = 64;

/// Policy shared by the durable writer and relay.
///
/// The deadline is intentionally absolute. Retrying a failed Valkey publish
/// does not extend it, which prevents a prolonged Valkey outage from turning
/// old intake into a later surprise launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionOutboxPolicy {
    pub request_deadline: Duration,
    pub lease_duration: Duration,
    pub retry_delay: Duration,
    pub poll_interval: Duration,
    pub batch_size: usize,
}

/// Bounded recovery policy for source reservations whose lifecycle callback
/// was interrupted.  This never scans Environment's instance table: it reads
/// only the small set of locally unreleased source reservations and asks the
/// runtime about those exact instance IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionAdmissionReconcilerConfig {
    pub interval: Duration,
    pub batch_size: usize,
}

impl Default for ExecutionAdmissionReconcilerConfig {
    fn default() -> Self {
        Self {
            interval: duration_from_env(
                "EXECUTION_ADMISSION_RECONCILE_INTERVAL_SECS",
                DEFAULT_ADMISSION_RECONCILE_INTERVAL,
                Duration::from_secs(1),
                Duration::from_secs(5 * 60),
                1_000,
            ),
            batch_size: usize_from_env(
                "EXECUTION_ADMISSION_RECONCILE_BATCH_SIZE",
                DEFAULT_ADMISSION_RECONCILE_BATCH_SIZE,
                1,
                500,
            ),
        }
    }
}

impl Default for ExecutionOutboxPolicy {
    fn default() -> Self {
        Self {
            request_deadline: duration_from_env(
                "EXECUTION_OUTBOX_DEADLINE_SECS",
                DEFAULT_OUTBOX_DEADLINE,
                Duration::from_secs(1),
                Duration::from_secs(24 * 60 * 60),
                1_000,
            ),
            lease_duration: duration_from_env(
                "EXECUTION_OUTBOX_LEASE_SECS",
                DEFAULT_OUTBOX_LEASE,
                Duration::from_secs(1),
                Duration::from_secs(5 * 60),
                1_000,
            ),
            retry_delay: duration_from_env(
                "EXECUTION_OUTBOX_RETRY_MS",
                DEFAULT_OUTBOX_RETRY_DELAY,
                Duration::from_millis(50),
                Duration::from_secs(60),
                1,
            ),
            poll_interval: duration_from_env(
                "EXECUTION_OUTBOX_POLL_MS",
                DEFAULT_OUTBOX_POLL_INTERVAL,
                Duration::from_millis(25),
                Duration::from_secs(10),
                1,
            ),
            batch_size: usize_from_env(
                "EXECUTION_OUTBOX_BATCH_SIZE",
                DEFAULT_OUTBOX_BATCH_SIZE,
                1,
                500,
            ),
        }
    }
}

fn duration_from_env(
    name: &str,
    default: Duration,
    min: Duration,
    max: Duration,
    multiplier: u64,
) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .and_then(|value| value.checked_mul(multiplier))
        .map(Duration::from_millis)
        .filter(|value| *value >= min && *value <= max)
        .unwrap_or(default)
}

fn usize_from_env(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value >= min && *value <= max)
        .unwrap_or(default)
}

/// Stable key namespace for sources that have a naturally replayable event
/// identity (cron ticks, caller-provided idempotency keys, etc.).
pub fn source_idempotency_key(source: &str, identity: &str) -> String {
    format!("{source}:{identity}")
}

/// A result returned by the durable source transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueuedExecution {
    pub request_id: Uuid,
    pub instance_id: String,
    /// `true` when the request was already committed by an earlier delivery of
    /// the same source identity. No additional admission was consumed.
    pub duplicate: bool,
}

/// Result of atomically taking a stream-delivered source request into the
/// Environment-launch handoff.
///
/// The short lease protects the deadline fence from a trigger worker crash.
/// It is independent of Environment's longer-lived durable launch ownership:
/// once Environment accepts the launch, [`ExecutionOutbox::mark_launch_accepted`]
/// makes that ownership durable in the server-side source record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableLaunchClaim {
    /// This worker owns the bounded handoff and may call Environment.
    Claimed,
    /// Environment already accepted this request on an earlier delivery.
    AlreadyAccepted,
    /// Another relay/worker has an active handoff lease. Leave the Valkey
    /// entry pending; a lease expiry is recoverable.
    InProgress,
    /// The source deadline elapsed before an Environment handoff was accepted.
    Expired,
    /// The request is no longer eligible (cancelled, terminalized, or does
    /// not match the tenant/instance carried by the stream message).
    Rejected,
}

/// What became of a relay's attempt to record its own stream delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMark {
    /// This relay recorded the delivery itself.
    Marked,
    /// A consumer claimed the launch handoff between this relay's XADD and
    /// its delivery commit. The publish still succeeded; the next durable
    /// stage simply got there first.
    AlreadyClaimed,
    /// The source record was expired or cancelled while the relay published,
    /// so this delivery no longer counts for anything.
    Lost,
}

#[derive(Debug, Error)]
pub enum ExecutionOutboxError {
    #[error("execution admission is full (limit {limit})")]
    AdmissionFull { limit: u64 },
    #[error("idempotency key is required and must not exceed 512 bytes")]
    InvalidIdempotencyKey,
    #[error("trigger event tenant does not match the enqueue tenant")]
    TenantMismatch,
    #[error("failed to serialize trigger event: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("execution outbox database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, FromRow)]
struct ExistingRequest {
    request_id: Uuid,
    instance_id: String,
}

/// Server-owned database boundary for accepted asynchronous executions.
#[derive(Clone)]
pub struct ExecutionOutbox {
    pool: PgPool,
    policy: ExecutionOutboxPolicy,
}

impl ExecutionOutbox {
    pub fn new(pool: PgPool) -> Self {
        Self::with_policy(pool, ExecutionOutboxPolicy::default())
    }

    pub fn with_policy(pool: PgPool, policy: ExecutionOutboxPolicy) -> Self {
        Self { pool, policy }
    }

    pub fn policy(&self) -> ExecutionOutboxPolicy {
        self.policy
    }

    /// Read an existing request before a caller performs non-durable admission
    /// work. This makes client retries a no-op even when the original request
    /// has already reached the worker stream.
    pub async fn find_by_idempotency(
        &self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<EnqueuedExecution>, ExecutionOutboxError> {
        let request = sqlx::query_as::<_, ExistingRequest>(
            r#"
            SELECT request_id, instance_id
            FROM execution_requests
            WHERE tenant_id = $1 AND idempotency_key = $2
            "#,
        )
        .bind(tenant_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(request.map(|request| EnqueuedExecution {
            request_id: request.request_id,
            instance_id: request.instance_id,
            duplicate: true,
        }))
    }

    /// Atomically records a request, its source-admission reservation, and
    /// one pending outbox delivery.
    ///
    /// `admission_limit` is evaluated by the caller using the same configured
    /// cap as the runtime gate. The counter update itself is transactional and
    /// serializes concurrent HTTP/event/cron writers in PostgreSQL.
    pub async fn enqueue(
        &self,
        tenant_id: &str,
        event: &TriggerEvent,
        idempotency_key: &str,
        admission_limit: u64,
    ) -> Result<EnqueuedExecution, ExecutionOutboxError> {
        if tenant_id != event.tenant_id {
            return Err(ExecutionOutboxError::TenantMismatch);
        }
        if idempotency_key.trim().is_empty() || idempotency_key.len() > 512 {
            return Err(ExecutionOutboxError::InvalidIdempotencyKey);
        }

        let payload = serde_json::to_value(event)?;
        let mut tx = self.pool.begin().await?;

        // A lookup alone is not enough: two identical source deliveries can
        // both observe no row before either inserts. The transaction-scoped
        // advisory lock makes that idempotency race deterministic while still
        // allowing unrelated tenants/requests to enqueue concurrently.
        let lock_key = format!("{tenant_id}\u{1f}{idempotency_key}");
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(lock_key)
            .execute(&mut *tx)
            .await?;

        if let Some(existing) =
            find_by_idempotency_in_tx(&mut tx, tenant_id, idempotency_key).await?
        {
            tx.commit().await?;
            return Ok(EnqueuedExecution {
                request_id: existing.request_id,
                instance_id: existing.instance_id,
                duplicate: true,
            });
        }

        let max_reservations =
            i64::try_from(admission_limit).map_err(|_| ExecutionOutboxError::AdmissionFull {
                limit: admission_limit,
            })?;
        if max_reservations <= 0 {
            return Err(ExecutionOutboxError::AdmissionFull {
                limit: admission_limit,
            });
        }

        sqlx::query(
            r#"
            INSERT INTO execution_admission_tenants (tenant_id)
            VALUES ($1)
            ON CONFLICT (tenant_id) DO NOTHING
            "#,
        )
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;

        let reserved = sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE execution_admission_tenants
            SET reserved_count = reserved_count + 1, updated_at = NOW()
            WHERE tenant_id = $1 AND reserved_count < $2
            RETURNING reserved_count
            "#,
        )
        .bind(tenant_id)
        .bind(max_reservations)
        .fetch_optional(&mut *tx)
        .await?;
        if reserved.is_none() {
            // Dropping the transaction rolls back the tenant-row creation too
            // when it was only created for this rejected request.
            return Err(ExecutionOutboxError::AdmissionFull {
                limit: admission_limit,
            });
        }

        let request_id = Uuid::new_v4();
        let deadline_at = deadline_from(Utc::now(), self.policy.request_deadline);
        sqlx::query(
            r#"
            INSERT INTO execution_requests (
                request_id, tenant_id, idempotency_key, instance_id, workflow_id,
                workflow_version, trigger_event, deadline_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(request_id)
        .bind(tenant_id)
        .bind(idempotency_key)
        .bind(&event.instance_id)
        .bind(&event.workflow_id)
        .bind(event.version)
        .bind(sqlx::types::Json(payload))
        .bind(deadline_at)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO execution_admission_reservations (request_id, tenant_id)
            VALUES ($1, $2)
            "#,
        )
        .bind(request_id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query("INSERT INTO execution_outbox (request_id) VALUES ($1)")
            .bind(request_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(EnqueuedExecution {
            request_id,
            instance_id: event.instance_id.clone(),
            duplicate: false,
        })
    }

    /// Release a reservation exactly once. P0.1's durable launch handoff and
    /// lifecycle transitions can call this without knowing whether a prior
    /// relay, expiry reaper, or terminal path already released it.
    pub async fn release_admission(
        &self,
        request_id: Uuid,
        reason: &str,
    ) -> Result<bool, ExecutionOutboxError> {
        let mut tx = self.pool.begin().await?;
        let released = release_admission_in_tx(&mut tx, request_id, reason).await?;
        tx.commit().await?;
        Ok(released)
    }

    /// Release by the stable execution identity for runtime paths that have an
    /// instance ID but did not retain the request ID. This is intentionally
    /// tenant-scoped, so equal IDs from separate tenant environments cannot
    /// affect one another.
    pub async fn release_admission_for_instance(
        &self,
        tenant_id: &str,
        instance_id: &str,
        reason: &str,
    ) -> Result<bool, ExecutionOutboxError> {
        let request_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT request_id
            FROM execution_requests
            WHERE tenant_id = $1 AND instance_id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;

        match request_id {
            Some(request_id) => self.release_admission(request_id, reason).await,
            None => Ok(false),
        }
    }

    /// Atomically fence a stream consumer before it asks Environment to create
    /// a durable launch. A stale stream entry cannot start a request after its
    /// source deadline: database time checks the deadline in the same update
    /// that records this worker's short handoff lease.
    ///
    /// The trigger worker rejects and ACKs legacy stream entries without a
    /// `request_id`; every launchable server source therefore reaches this
    /// durable fence.
    pub async fn claim_for_launch(
        &self,
        request_id: Uuid,
        tenant_id: &str,
        instance_id: &str,
        lease_owner: &str,
    ) -> Result<DurableLaunchClaim, ExecutionOutboxError> {
        let lease_ms = i64::try_from(self.policy.lease_duration.as_millis()).unwrap_or(i64::MAX);

        // A handoff lease can expire after a worker crashes between receiving
        // the stream entry and Environment's durable accept. Recover it once
        // in this caller rather than requiring an unrelated sweep before the
        // PEL retry can make progress.
        for _ in 0..2 {
            let mut tx = self.pool.begin().await?;
            // Take the outbox row before the request row. The relay's delivery
            // commit locks them in that order, and the claim below locks them
            // in the opposite one, so the two deadlock whenever they touch the
            // same request — which is the ordinary case now that a consumer
            // may claim inside the relay's delivery window. Ordering both
            // paths the same way turns that race into a short wait.
            sqlx::query("SELECT 1 FROM execution_outbox WHERE request_id = $1 FOR UPDATE")
                .bind(request_id)
                .execute(&mut *tx)
                .await?;
            let claimed = sqlx::query_scalar::<_, Uuid>(
                r#"
                WITH claimed AS (
                    UPDATE execution_requests AS request
                    SET state = 'launching', updated_at = NOW()
                    FROM execution_outbox AS outbox
                    WHERE request.request_id = $1
                      AND outbox.request_id = request.request_id
                      AND request.tenant_id = $2
                      AND request.instance_id = $3
                      -- Either the relay's delivery commit already landed, or
                      -- this caller is inside the window between the relay's
                      -- XADD and that commit. Holding the stream entry that
                      -- carries this request_id is itself proof the relay
                      -- published it, so the still-`leased` row is claimable:
                      -- waiting for the commit would strand the entry in the
                      -- PEL until an idle-based reclaim, adding that delay to
                      -- every launch.
                      AND (
                            (request.state = 'delivered' AND outbox.state = 'delivered')
                         OR (request.state = 'queued' AND outbox.state = 'leased')
                      )
                      AND request.deadline_at > NOW()
                    RETURNING request.request_id
                )
                UPDATE execution_outbox AS outbox
                SET state = 'leased',
                    lease_owner = $4,
                    lease_expires_at = NOW() + ($5 * INTERVAL '1 millisecond'),
                    attempt_count = outbox.attempt_count + 1,
                    updated_at = NOW()
                FROM claimed
                WHERE outbox.request_id = claimed.request_id
                RETURNING outbox.request_id
                "#,
            )
            .bind(request_id)
            .bind(tenant_id)
            .bind(instance_id)
            .bind(lease_owner)
            .bind(lease_ms)
            .fetch_optional(&mut *tx)
            .await?;
            if claimed.is_some() {
                tx.commit().await?;
                return Ok(DurableLaunchClaim::Claimed);
            }

            let row = sqlx::query_as::<_, LaunchHandoffRow>(
                r#"
                SELECT request.tenant_id,
                       request.instance_id,
                       request.state AS request_state,
                       outbox.state AS outbox_state,
                       request.deadline_at <= NOW() AS deadline_elapsed,
                       COALESCE(outbox.lease_expires_at <= NOW(), false) AS lease_expired
                FROM execution_requests AS request
                INNER JOIN execution_outbox AS outbox
                    ON outbox.request_id = request.request_id
                WHERE request.request_id = $1
                FOR UPDATE OF request, outbox
                "#,
            )
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await?;

            let Some(row) = row else {
                tx.commit().await?;
                return Ok(DurableLaunchClaim::Rejected);
            };
            if row.tenant_id != tenant_id || row.instance_id != instance_id {
                tx.commit().await?;
                return Ok(DurableLaunchClaim::Rejected);
            }
            if row.request_state == "accepted" {
                tx.commit().await?;
                return Ok(DurableLaunchClaim::AlreadyAccepted);
            }
            if matches!(
                row.request_state.as_str(),
                "expired" | "cancelled" | "terminal"
            ) {
                tx.commit().await?;
                return Ok(if row.request_state == "expired" {
                    DurableLaunchClaim::Expired
                } else {
                    DurableLaunchClaim::Rejected
                });
            }

            // A source request that reached its deadline before Environment
            // accepted it is terminalized and releases its counter right now.
            // A live launch handoff lease gets to finish: it had already
            // crossed the source fence before expiry, and Environment's own
            // queue deadline now owns the next bounded stage.
            let handoff_lease_is_live = row.request_state == "launching"
                && row.outbox_state == "leased"
                && !row.lease_expired;
            if row.deadline_elapsed && !handoff_lease_is_live {
                expire_request_in_tx(&mut tx, request_id).await?;
                tx.commit().await?;
                return Ok(DurableLaunchClaim::Expired);
            }

            if row.request_state == "launching" && row.outbox_state == "leased" && row.lease_expired
            {
                // The prior worker died before a durable Environment accept.
                // Reset to the stream-delivered state, then take it again in
                // the next loop iteration while the transaction lock is gone.
                reset_launch_handoff_in_tx(&mut tx, request_id).await?;
                tx.commit().await?;
                continue;
            }

            tx.commit().await?;
            return Ok(DurableLaunchClaim::InProgress);
        }

        // The loop can only exhaust after another worker repeatedly wins the
        // lease between recovery and this worker's retry. Keeping the stream
        // entry pending is safer than launching without a durable fence.
        Ok(DurableLaunchClaim::InProgress)
    }

    /// Commit that Environment durably accepted the source request. This does
    /// not release source admission: the lifecycle observer/reconciler does
    /// that only after the launch parks, terminalizes, expires, or cancels.
    pub async fn mark_launch_accepted(
        &self,
        request_id: Uuid,
        lease_owner: &str,
    ) -> Result<bool, ExecutionOutboxError> {
        let mut tx = self.pool.begin().await?;
        let accepted = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE execution_requests AS request
            SET state = 'accepted', updated_at = NOW()
            FROM execution_outbox AS outbox
            WHERE request.request_id = $1
              AND outbox.request_id = request.request_id
              AND request.state = 'launching'
              AND outbox.state = 'leased'
              AND outbox.lease_owner = $2
            RETURNING request.request_id
            "#,
        )
        .bind(request_id)
        .bind(lease_owner)
        .fetch_optional(&mut *tx)
        .await?;

        if accepted.is_some() {
            sqlx::query(
                r#"
                UPDATE execution_outbox
                SET state = 'delivered',
                    lease_owner = NULL,
                    lease_expires_at = NULL,
                    updated_at = NOW()
                WHERE request_id = $1 AND state = 'leased' AND lease_owner = $2
                "#,
            )
            .bind(request_id)
            .bind(lease_owner)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(true);
        }

        let already_accepted = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM execution_requests WHERE request_id = $1 AND state = 'accepted')",
        )
        .bind(request_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(already_accepted)
    }

    /// Return a handoff owned by this worker to the stream-delivered state so
    /// the pending Valkey entry can retry (for example while compilation is
    /// still in progress). A cancellation or expiry that won the race cannot
    /// be resurrected by this transition.
    pub async fn release_launch_claim(
        &self,
        request_id: Uuid,
        lease_owner: &str,
        error_message: &str,
    ) -> Result<bool, ExecutionOutboxError> {
        let mut tx = self.pool.begin().await?;
        let released = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE execution_requests AS request
            SET state = 'delivered', updated_at = NOW()
            FROM execution_outbox AS outbox
            WHERE request.request_id = $1
              AND outbox.request_id = request.request_id
              AND request.state = 'launching'
              AND outbox.state = 'leased'
              AND outbox.lease_owner = $2
            RETURNING request.request_id
            "#,
        )
        .bind(request_id)
        .bind(lease_owner)
        .fetch_optional(&mut *tx)
        .await?;
        if released.is_some() {
            sqlx::query(
                r#"
                UPDATE execution_outbox
                SET state = 'delivered',
                    lease_owner = NULL,
                    lease_expires_at = NULL,
                    last_error = $3,
                    updated_at = NOW()
                WHERE request_id = $1 AND state = 'leased' AND lease_owner = $2
                "#,
            )
            .bind(request_id)
            .bind(lease_owner)
            .bind(error_message)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(released.is_some())
    }

    /// Terminalize a request that cannot reach Environment after a worker
    /// owns its handoff (invalid workflow, deliberate single-instance skip,
    /// etc.). This clears the source reservation in the same transaction.
    pub async fn terminalize_launch_claim(
        &self,
        request_id: Uuid,
        lease_owner: &str,
        reason: &str,
    ) -> Result<bool, ExecutionOutboxError> {
        let mut tx = self.pool.begin().await?;
        let terminalized = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE execution_requests AS request
            SET state = 'terminal', terminal_reason = $3, updated_at = NOW()
            FROM execution_outbox AS outbox
            WHERE request.request_id = $1
              AND outbox.request_id = request.request_id
              AND request.state = 'launching'
              AND outbox.state = 'leased'
              AND outbox.lease_owner = $2
            RETURNING request.request_id
            "#,
        )
        .bind(request_id)
        .bind(lease_owner)
        .bind(reason)
        .fetch_optional(&mut *tx)
        .await?;
        if terminalized.is_some() {
            sqlx::query(
                r#"
                UPDATE execution_outbox
                SET state = 'cancelled',
                    lease_owner = NULL,
                    lease_expires_at = NULL,
                    last_error = $3,
                    updated_at = NOW()
                WHERE request_id = $1 AND state = 'leased' AND lease_owner = $2
                "#,
            )
            .bind(request_id)
            .bind(lease_owner)
            .bind(reason)
            .execute(&mut *tx)
            .await?;
            release_admission_in_tx(&mut tx, request_id, reason).await?;
        }
        tx.commit().await?;
        Ok(terminalized.is_some())
    }

    /// Return a bounded set of stream-delivered or Environment-accepted
    /// requests that still own source admission. Before relay delivery there
    /// is no Environment instance to inspect, and the absolute outbox
    /// deadline is responsible for release instead.
    async fn unreleased_delivered_reservations(
        &self,
        limit: usize,
    ) -> Result<Vec<ActiveAdmissionReservation>, ExecutionOutboxError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        Ok(sqlx::query_as::<_, ActiveAdmissionReservation>(
            r#"
            SELECT reservation.request_id, request.tenant_id, request.instance_id
            FROM execution_admission_reservations AS reservation
            INNER JOIN execution_requests AS request
                ON request.request_id = reservation.request_id
            WHERE reservation.released_at IS NULL
              AND request.state IN ('delivered', 'accepted')
            ORDER BY reservation.created_at
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn claim_batch(
        &self,
        worker_id: &str,
    ) -> Result<Vec<ClaimedOutbox>, ExecutionOutboxError> {
        let lease_ms = i64::try_from(self.policy.lease_duration.as_millis()).unwrap_or(i64::MAX);
        let batch_size = i64::try_from(self.policy.batch_size).unwrap_or(i64::MAX);
        let rows = sqlx::query_as::<_, ClaimedOutbox>(
            r#"
            WITH candidates AS (
                SELECT o.request_id
                FROM execution_outbox AS o
                INNER JOIN execution_requests AS r ON r.request_id = o.request_id
                WHERE r.state = 'queued'
                  AND r.deadline_at > NOW()
                  AND (
                    (o.state = 'pending' AND o.available_at <= NOW())
                    OR (o.state = 'leased' AND o.lease_expires_at <= NOW())
                  )
                ORDER BY o.available_at, o.created_at
                FOR UPDATE OF o SKIP LOCKED
                LIMIT $2
            )
            UPDATE execution_outbox AS o
            SET state = 'leased',
                lease_owner = $1,
                lease_expires_at = NOW() + ($3 * INTERVAL '1 millisecond'),
                attempt_count = o.attempt_count + 1,
                updated_at = NOW()
            FROM candidates AS c
            INNER JOIN execution_requests AS r ON r.request_id = c.request_id
            WHERE o.request_id = c.request_id
            RETURNING o.request_id, r.tenant_id, r.trigger_event, r.deadline_at
            "#,
        )
        .bind(worker_id)
        .bind(batch_size)
        .bind(lease_ms)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn mark_delivered(
        &self,
        request_id: Uuid,
        worker_id: &str,
        stream_id: &str,
    ) -> Result<DeliveryMark, ExecutionOutboxError> {
        let mut tx = self.pool.begin().await?;
        let delivered = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE execution_outbox AS o
            SET state = 'delivered',
                stream_id = $3,
                delivered_at = NOW(),
                lease_owner = NULL,
                lease_expires_at = NULL,
                updated_at = NOW()
            FROM execution_requests AS r
            WHERE o.request_id = $1
              AND o.request_id = r.request_id
              AND o.state = 'leased'
              AND o.lease_owner = $2
              AND r.state = 'queued'
              AND r.deadline_at > NOW()
            RETURNING o.request_id
            "#,
        )
        .bind(request_id)
        .bind(worker_id)
        .bind(stream_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(request_id) = delivered else {
            // A consumer can claim the launch handoff inside the window
            // between the XADD and this commit, which leaves nothing for the
            // update above to match. That is a completed delivery, not a lost
            // one: stamp the stream id for traceability and say so.
            let handed_off = sqlx::query_scalar::<_, Uuid>(
                r#"
                UPDATE execution_outbox AS o
                SET stream_id = COALESCE(o.stream_id, $2),
                    delivered_at = COALESCE(o.delivered_at, NOW()),
                    updated_at = NOW()
                FROM execution_requests AS r
                WHERE o.request_id = $1
                  AND o.request_id = r.request_id
                  AND r.state IN ('launching', 'accepted')
                RETURNING o.request_id
                "#,
            )
            .bind(request_id)
            .bind(stream_id)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(if handed_off.is_some() {
                DeliveryMark::AlreadyClaimed
            } else {
                DeliveryMark::Lost
            });
        };

        sqlx::query(
            r#"
            UPDATE execution_requests
            SET state = 'delivered', delivered_at = NOW(), updated_at = NOW()
            WHERE request_id = $1 AND state = 'queued'
            "#,
        )
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
        // Do *not* release admission here. Stream delivery merely transfers a
        // request to the next durable stage; it is not proof that a launch
        // has parked or terminalized. P0.1 receives `request_id` in the event
        // and calls the explicit release hook only when its launch lifecycle
        // owns that transition.
        tx.commit().await?;
        Ok(DeliveryMark::Marked)
    }

    async fn release_claim(
        &self,
        request_id: Uuid,
        worker_id: &str,
        error_message: &str,
    ) -> Result<bool, ExecutionOutboxError> {
        let retry_ms = i64::try_from(self.policy.retry_delay.as_millis()).unwrap_or(i64::MAX);
        let released = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE execution_outbox AS o
            SET state = 'pending',
                available_at = NOW() + ($3 * INTERVAL '1 millisecond'),
                lease_owner = NULL,
                lease_expires_at = NULL,
                last_error = $4,
                updated_at = NOW()
            FROM execution_requests AS r
            WHERE o.request_id = $1
              AND o.request_id = r.request_id
              AND o.state = 'leased'
              AND o.lease_owner = $2
              AND r.state = 'queued'
              AND r.deadline_at > NOW()
            RETURNING o.request_id
            "#,
        )
        .bind(request_id)
        .bind(worker_id)
        .bind(retry_ms)
        .bind(error_message)
        .fetch_optional(&self.pool)
        .await?;
        Ok(released.is_some())
    }

    /// Terminalize source records whose absolute delivery deadline elapsed.
    /// The update wins over an expired relay or trigger-worker handoff lease
    /// and releases the durable counter in the same database transaction.
    /// `accepted` requests are intentionally excluded: Environment has already
    /// taken durable ownership, so its own bounded launch deadline applies.
    pub async fn expire_due(&self) -> Result<usize, ExecutionOutboxError> {
        let mut tx = self.pool.begin().await?;
        let request_ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE execution_outbox AS o
            SET state = 'expired',
                lease_owner = NULL,
                lease_expires_at = NULL,
                last_error = 'execution_outbox_deadline_exceeded',
                updated_at = NOW()
            FROM execution_requests AS r
            WHERE o.request_id = r.request_id
              AND r.deadline_at <= NOW()
              AND (
                    (r.state = 'queued' AND o.state IN ('pending', 'leased'))
                 OR (r.state = 'delivered' AND o.state = 'delivered')
                 OR (
                        r.state = 'launching'
                    AND o.state = 'leased'
                    AND o.lease_expires_at <= NOW()
                 )
              )
            RETURNING o.request_id
            "#,
        )
        .fetch_all(&mut *tx)
        .await?;

        for request_id in &request_ids {
            sqlx::query(
                r#"
                UPDATE execution_requests
                SET state = 'expired',
                    terminal_reason = 'execution_outbox_deadline_exceeded',
                    updated_at = NOW()
                WHERE request_id = $1
                  AND state IN ('queued', 'delivered', 'launching')
                "#,
            )
            .bind(request_id)
            .execute(&mut *tx)
            .await?;
            release_admission_in_tx(&mut tx, *request_id, "execution_outbox_deadline_exceeded")
                .await?;
        }

        tx.commit().await?;
        Ok(request_ids.len())
    }

    /// Recover a source handoff whose trigger worker died before Environment
    /// accepted the launch. The row returns to the relay rather than assuming
    /// the old Valkey PEL survived a broker restart; duplicate stream entries
    /// are safe because `instance_id` remains the Environment idempotency key.
    async fn recover_expired_launch_claims(&self) -> Result<usize, ExecutionOutboxError> {
        let mut tx = self.pool.begin().await?;
        let request_ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE execution_outbox AS outbox
            SET state = 'pending',
                available_at = NOW(),
                lease_owner = NULL,
                lease_expires_at = NULL,
                last_error = 'execution_launch_handoff_lease_expired',
                updated_at = NOW()
            FROM execution_requests AS request
            WHERE outbox.request_id = request.request_id
              AND request.state = 'launching'
              AND outbox.state = 'leased'
              AND outbox.lease_expires_at <= NOW()
              AND request.deadline_at > NOW()
            RETURNING outbox.request_id
            "#,
        )
        .fetch_all(&mut *tx)
        .await?;

        if !request_ids.is_empty() {
            sqlx::query(
                r#"
                UPDATE execution_requests
                SET state = 'queued', updated_at = NOW()
                WHERE request_id = ANY($1) AND state = 'launching'
                "#,
            )
            .bind(&request_ids)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(request_ids.len())
    }
}

async fn find_by_idempotency_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    idempotency_key: &str,
) -> Result<Option<ExistingRequest>, sqlx::Error> {
    sqlx::query_as::<_, ExistingRequest>(
        r#"
        SELECT request_id, instance_id
        FROM execution_requests
        WHERE tenant_id = $1 AND idempotency_key = $2
        "#,
    )
    .bind(tenant_id)
    .bind(idempotency_key)
    .fetch_optional(&mut **tx)
    .await
}

async fn release_admission_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
    reason: &str,
) -> Result<bool, sqlx::Error> {
    let tenant_id = sqlx::query_scalar::<_, String>(
        r#"
        UPDATE execution_admission_reservations
        SET released_at = NOW(), release_reason = $2
        WHERE request_id = $1 AND released_at IS NULL
        RETURNING tenant_id
        "#,
    )
    .bind(request_id)
    .bind(reason)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(tenant_id) = tenant_id else {
        return Ok(false);
    };

    sqlx::query(
        r#"
        UPDATE execution_admission_tenants
        SET reserved_count = GREATEST(reserved_count - 1, 0), updated_at = NOW()
        WHERE tenant_id = $1
        "#,
    )
    .bind(tenant_id)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}

async fn expire_request_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE execution_outbox
        SET state = 'expired',
            lease_owner = NULL,
            lease_expires_at = NULL,
            last_error = 'execution_outbox_deadline_exceeded',
            updated_at = NOW()
        WHERE request_id = $1
          AND state IN ('pending', 'leased', 'delivered')
        "#,
    )
    .bind(request_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE execution_requests
        SET state = 'expired',
            terminal_reason = 'execution_outbox_deadline_exceeded',
            updated_at = NOW()
        WHERE request_id = $1
          AND state IN ('queued', 'delivered', 'launching')
        "#,
    )
    .bind(request_id)
    .execute(&mut **tx)
    .await?;
    release_admission_in_tx(tx, request_id, "execution_outbox_deadline_exceeded").await?;
    Ok(())
}

async fn reset_launch_handoff_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE execution_outbox
        SET state = 'delivered',
            lease_owner = NULL,
            lease_expires_at = NULL,
            last_error = 'execution_launch_handoff_lease_expired',
            updated_at = NOW()
        WHERE request_id = $1 AND state = 'leased'
        "#,
    )
    .bind(request_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE execution_requests
        SET state = 'delivered', updated_at = NOW()
        WHERE request_id = $1 AND state = 'launching'
        "#,
    )
    .bind(request_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn deadline_from(now: DateTime<Utc>, duration: Duration) -> DateTime<Utc> {
    now + ChronoDuration::from_std(duration).unwrap_or_else(|_| ChronoDuration::days(1))
}

#[derive(Debug, FromRow)]
struct ClaimedOutbox {
    request_id: Uuid,
    tenant_id: String,
    trigger_event: Value,
    #[allow(dead_code)]
    deadline_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct LaunchHandoffRow {
    tenant_id: String,
    instance_id: String,
    request_state: String,
    outbox_state: String,
    deadline_elapsed: bool,
    lease_expired: bool,
}

#[derive(Debug, Clone, FromRow)]
struct ActiveAdmissionReservation {
    request_id: Uuid,
    tenant_id: String,
    instance_id: String,
}

/// Summary of one bounded admission-reconciliation pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionReconcileStats {
    pub inspected: usize,
    pub released: usize,
    pub unresolved: usize,
}

/// Repairs a missed launch lifecycle callback after a process crash.
///
/// The normal P0.1 lifecycle observer releases an admission reservation in
/// the same post-commit path that parks, expires, cancels, or terminalizes a
/// durable launch.  That observer is intentionally best-effort with respect
/// to this separate server database, so this bounded reconciler is the retry
/// mechanism: it only releases when Environment has already made the
/// instance suspended or terminal, and doing so is idempotent.
#[derive(Clone)]
pub struct ExecutionAdmissionReconciler {
    outbox: ExecutionOutbox,
    runtime_client: Arc<RuntimeClient>,
    config: ExecutionAdmissionReconcilerConfig,
}

/// Environment-to-server bridge for the durable launch lifecycle.
///
/// Environment invokes this only after it commits a parked, terminal,
/// cancelled, or expired launch transition. The server database remains a
/// separate transactional boundary, so failures are surfaced to Environment's
/// observer holder and retried by [`ExecutionAdmissionReconciler`].
#[derive(Clone)]
pub struct ExecutionAdmissionLifecycleObserver {
    outbox: ExecutionOutbox,
}

impl ExecutionAdmissionLifecycleObserver {
    pub fn new(outbox: ExecutionOutbox) -> Self {
        Self { outbox }
    }
}

#[async_trait::async_trait]
impl runtara_environment::launch_dispatcher::LaunchLifecycleObserver
    for ExecutionAdmissionLifecycleObserver
{
    async fn release_admission(
        &self,
        tenant_id: &str,
        instance_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        match self
            .outbox
            .release_admission_for_instance(tenant_id, instance_id, reason)
            .await
        {
            Ok(true) => {
                info!(
                    tenant_id,
                    instance_id,
                    reason,
                    "Released durable execution admission after Environment lifecycle transition"
                );
                Ok(())
            }
            Ok(false) => {
                // The reservation may belong to a legacy instance or have
                // been released by the reconciler first. Both are expected
                // idempotent outcomes, not observer failures.
                debug!(
                    tenant_id,
                    instance_id,
                    reason,
                    "No active durable execution admission remained for lifecycle transition"
                );
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

impl ExecutionAdmissionReconciler {
    pub fn new(outbox: ExecutionOutbox, runtime_client: Arc<RuntimeClient>) -> Self {
        Self::with_config(
            outbox,
            runtime_client,
            ExecutionAdmissionReconcilerConfig::default(),
        )
    }

    pub fn with_config(
        outbox: ExecutionOutbox,
        runtime_client: Arc<RuntimeClient>,
        config: ExecutionAdmissionReconcilerConfig,
    ) -> Self {
        Self {
            outbox,
            runtime_client,
            config,
        }
    }

    /// Inspect only unreleased, stream-delivered or Environment-accepted
    /// source reservations. Missing or transiently unreadable instances
    /// remain reserved: an undelivered launch must never be mistaken for a
    /// suspended one.
    pub async fn run_once(&self) -> Result<AdmissionReconcileStats, ExecutionOutboxError> {
        let reservations = self
            .outbox
            .unreleased_delivered_reservations(self.config.batch_size)
            .await?;
        let mut stats = AdmissionReconcileStats {
            inspected: reservations.len(),
            ..AdmissionReconcileStats::default()
        };

        for reservation in reservations {
            match self
                .runtime_client
                .get_instance_status(&reservation.instance_id)
                .await
            {
                Ok(status) => {
                    let Some(reason) = release_reason_for_status(status) else {
                        stats.unresolved += 1;
                        continue;
                    };

                    match self
                        .outbox
                        .release_admission(reservation.request_id, reason)
                        .await
                    {
                        Ok(true) => {
                            stats.released += 1;
                            info!(
                                request_id = %reservation.request_id,
                                tenant_id = %reservation.tenant_id,
                                instance_id = %reservation.instance_id,
                                status = status.as_str(),
                                reason,
                                "Reconciled durable execution admission reservation"
                            );
                        }
                        Ok(false) => {
                            // The normal observer won the race. Its release is
                            // authoritative and the counter was decremented
                            // exactly once.
                        }
                        Err(error) => {
                            stats.unresolved += 1;
                            warn!(
                                request_id = %reservation.request_id,
                                tenant_id = %reservation.tenant_id,
                                instance_id = %reservation.instance_id,
                                error = %error,
                                "Failed to reconcile durable execution admission reservation"
                            );
                        }
                    }
                }
                Err(error) => {
                    stats.unresolved += 1;
                    warn!(
                        request_id = %reservation.request_id,
                        tenant_id = %reservation.tenant_id,
                        instance_id = %reservation.instance_id,
                        error = %error,
                        "Unable to inspect execution admission reservation; retaining it"
                    );
                }
            }
        }

        Ok(stats)
    }

    pub async fn run(self, shutdown: ShutdownSignal) {
        info!("Execution admission reconciler started");
        let mut interval = tokio::time::interval(self.config.interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = wait_for_shutdown(&shutdown) => {
                    info!("Execution admission reconciler stopping on shutdown");
                    return;
                }
            }
            if shutdown.is_shutting_down() {
                info!("Execution admission reconciler stopping on shutdown");
                return;
            }
            match self.run_once().await {
                Ok(stats) if stats.inspected > 0 => {
                    debug!(?stats, "Execution admission reconciliation pass completed");
                }
                Ok(_) => {}
                Err(error) => {
                    error!(error = %error, "Execution admission reconciliation pass failed");
                }
            }
        }
    }
}

fn release_reason_for_status(status: InstanceStatus) -> Option<&'static str> {
    match status {
        InstanceStatus::Suspended => Some("runtime_suspended"),
        InstanceStatus::Completed => Some("runtime_completed"),
        InstanceStatus::Failed => Some("runtime_failed"),
        InstanceStatus::Cancelled => Some("runtime_cancelled"),
        InstanceStatus::Unknown | InstanceStatus::Pending | InstanceStatus::Running => None,
    }
}

/// One relay pass, useful for observability and focused integration tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RelayRunStats {
    pub expired: usize,
    pub recovered: usize,
    pub claimed: usize,
    pub delivered: usize,
    pub failed: usize,
}

/// Relays durable outbox records into the existing Valkey trigger stream.
#[derive(Clone)]
pub struct ExecutionOutboxRelay {
    outbox: ExecutionOutbox,
    publisher: Arc<TriggerStreamPublisher>,
    worker_id: String,
}

impl ExecutionOutboxRelay {
    pub fn new(outbox: ExecutionOutbox, publisher: Arc<TriggerStreamPublisher>) -> Self {
        Self {
            outbox,
            publisher,
            worker_id: format!("execution-outbox-{}", Uuid::new_v4()),
        }
    }

    pub async fn run_once(&self) -> Result<RelayRunStats, ExecutionOutboxError> {
        let expired = self.outbox.expire_due().await?;
        let recovered = self.outbox.recover_expired_launch_claims().await?;
        let claimed = self.outbox.claim_batch(&self.worker_id).await?;
        let mut stats = RelayRunStats {
            expired,
            recovered,
            claimed: claimed.len(),
            ..RelayRunStats::default()
        };

        for claim in claimed {
            let mut event = match serde_json::from_value::<TriggerEvent>(claim.trigger_event) {
                Ok(event) => event,
                Err(error) => {
                    // The writer serializes the same type, so this represents
                    // data corruption rather than a user payload failure. Keep
                    // the row durable/retryable and surface it loudly instead
                    // of silently dropping accepted work.
                    let message = format!("failed to decode stored trigger event: {error}");
                    error!(request_id = %claim.request_id, error = %error, "Execution outbox payload is invalid");
                    self.outbox
                        .release_claim(claim.request_id, &self.worker_id, &message)
                        .await?;
                    stats.failed += 1;
                    continue;
                }
            };
            event.request_id = Some(claim.request_id);

            match self
                .publisher
                .publish_with_request_id(&claim.tenant_id, &event, claim.request_id)
                .await
            {
                Ok(stream_id) => {
                    match self
                        .outbox
                        .mark_delivered(claim.request_id, &self.worker_id, &stream_id)
                        .await?
                    {
                        mark @ (DeliveryMark::Marked | DeliveryMark::AlreadyClaimed) => {
                            stats.delivered += 1;
                            debug!(
                                request_id = %claim.request_id,
                                stream_id = %stream_id,
                                instance_id = %event.instance_id,
                                already_claimed = mark == DeliveryMark::AlreadyClaimed,
                                "Delivered durable execution request to trigger stream"
                            );
                        }
                        DeliveryMark::Lost => {
                            // The request was expired/cancelled while this
                            // relay was publishing. The stream consumer still
                            // deduplicates by instance ID, but do not claim
                            // success for an obsolete source record.
                            warn!(
                                request_id = %claim.request_id,
                                "Execution outbox delivery lost its lease or deadline before it could be marked"
                            );
                        }
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    warn!(
                        request_id = %claim.request_id,
                        tenant_id = %claim.tenant_id,
                        error = %message,
                        "Failed to relay execution outbox record; keeping it durable for retry"
                    );
                    self.outbox
                        .release_claim(claim.request_id, &self.worker_id, &message)
                        .await?;
                    stats.failed += 1;
                }
            }
        }
        Ok(stats)
    }

    pub async fn run(self, shutdown: ShutdownSignal) {
        info!(worker_id = %self.worker_id, "Execution outbox relay started");
        let mut interval = tokio::time::interval(self.outbox.policy().poll_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = wait_for_shutdown(&shutdown) => {
                    info!(worker_id = %self.worker_id, "Execution outbox relay stopping on shutdown");
                    return;
                }
            }
            if shutdown.is_shutting_down() {
                info!(worker_id = %self.worker_id, "Execution outbox relay stopping on shutdown");
                return;
            }
            match self.run_once().await {
                Ok(stats) if stats.claimed > 0 || stats.expired > 0 || stats.recovered > 0 => {
                    debug!(?stats, "Execution outbox relay pass completed");
                }
                Ok(_) => {}
                Err(error) => {
                    error!(error = %error, "Execution outbox relay pass failed");
                }
            }
        }
    }
}

async fn wait_for_shutdown(shutdown: &ShutdownSignal) {
    loop {
        if shutdown.is_shutting_down() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_key_is_stable_and_namespaced() {
        assert_eq!(
            source_idempotency_key("cron", "trigger-1:1710000000000"),
            "cron:trigger-1:1710000000000"
        );
        assert_ne!(
            source_idempotency_key("cron", "same"),
            source_idempotency_key("http-event", "same")
        );
    }

    #[test]
    fn policy_rejects_out_of_range_duration_values() {
        assert_eq!(
            duration_from_env(
                "unused",
                Duration::from_secs(30),
                Duration::from_secs(1),
                Duration::from_secs(60),
                1_000,
            ),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn deadline_is_absolute_from_the_enqueue_instant() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(
            deadline_from(now, Duration::from_secs(15)),
            DateTime::from_timestamp(1_700_000_015, 0).unwrap()
        );
    }

    #[test]
    fn reconciliation_releases_only_parked_or_terminal_instances() {
        assert_eq!(
            release_reason_for_status(InstanceStatus::Suspended),
            Some("runtime_suspended")
        );
        assert_eq!(
            release_reason_for_status(InstanceStatus::Completed),
            Some("runtime_completed")
        );
        assert_eq!(
            release_reason_for_status(InstanceStatus::Failed),
            Some("runtime_failed")
        );
        assert_eq!(
            release_reason_for_status(InstanceStatus::Cancelled),
            Some("runtime_cancelled")
        );
        for active in [
            InstanceStatus::Unknown,
            InstanceStatus::Pending,
            InstanceStatus::Running,
        ] {
            assert_eq!(release_reason_for_status(active), None);
        }
    }
}
