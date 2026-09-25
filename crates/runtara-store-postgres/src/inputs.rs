//! Atomic managed waits. Every writer locks the tenant-owned root first.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use runtara_core::persistence::{inputs::*, invocations::AttemptState};
use serde_json::Value;
use sqlx::{PgConnection, Postgres, Transaction};

use crate::PostgresPersistence;

// Shared by paged discovery and batched flags. Logical attempts remain active
// across a resumable park even when the physical execution lease is revoked.
const ACTIONABLE_FILTER: &str = "r.tenant_id=$1 AND r.instance_id=ANY($2) AND r.state='open' AND (r.deadline IS NULL OR r.deadline>$3) AND i.status NOT IN ('completed','failed','cancelled') AND (r.invocation_path='' OR (SELECT a.state FROM invocation_attempts a WHERE a.instance_id=r.instance_id AND a.invocation_path=r.invocation_path ORDER BY generation DESC LIMIT 1)='active')";

fn storage(error: impl std::fmt::Display) -> InputError {
    InputError::Storage(error.to_string())
}

#[derive(sqlx::FromRow)]
struct RequestRow {
    tenant_id: String,
    instance_id: String,
    request_id: String,
    invocation_path: String,
    fence: Option<Value>,
    // Exact serialized text: JSONB would renumber floats and reject NUL escapes.
    spec: String,
    created_at: DateTime<Utc>,
    state: String,
    receipt_id: Option<uuid::Uuid>,
    operation_id: Option<String>,
    accepted_payload: Option<Vec<u8>>,
    acceptance_context: Option<Vec<u8>>,
    accepted_at: Option<DateTime<Utc>>,
    closure_reason: Option<String>,
    closed_at: Option<DateTime<Utc>>,
    wake_pending: bool,
}

fn required<T>(value: Option<T>) -> InputResult<T> {
    value.ok_or_else(|| storage("incomplete input request record"))
}

impl RequestRow {
    fn record(self) -> InputResult<InputRequest> {
        let state = match self.state.as_str() {
            "open" => InputState::Open,
            "accepted" => InputState::Accepted {
                receipt: InputReceipt {
                    receipt_id: required(self.receipt_id)?.to_string(),
                    operation_id: required(self.operation_id)?,
                    request_id: self.request_id.clone(),
                    accepted_at: required(self.accepted_at)?,
                    payload: required(self.accepted_payload)?,
                    acceptance_context: self.acceptance_context,
                },
            },
            "closed" => InputState::Closed {
                reason: serde_json::from_value(Value::String(required(self.closure_reason)?))
                    .map_err(storage)?,
                closed_at: required(self.closed_at)?,
            },
            _ => return Err(storage("invalid input request state")),
        };
        Ok(InputRequest {
            tenant_id: self.tenant_id,
            instance_id: self.instance_id,
            request_id: self.request_id,
            invocation_path: self.invocation_path,
            fence: self
                .fence
                .map(serde_json::from_value)
                .transpose()
                .map_err(storage)?,
            spec: serde_json::from_str(&self.spec).map_err(storage)?,
            created_at: self.created_at,
            state,
            wake_pending: self.wake_pending,
        })
    }
}

async fn root(db: &mut PgConnection, tenant: &str, instance: &str) -> InputResult<String> {
    sqlx::query_scalar(
        "SELECT status::text FROM instances WHERE instance_id=$1 AND tenant_id=$2 FOR UPDATE",
    )
    .bind(instance)
    .bind(tenant)
    .fetch_optional(db)
    .await
    .map_err(storage)?
    .ok_or(InputError::NotFound)
}

/// Unlocked read of a root's status inside a read-only snapshot. Writers keep
/// taking the root lock; readers never block lifecycle or checkpoint writes.
async fn root_snapshot(db: &mut PgConnection, tenant: &str, instance: &str) -> InputResult<()> {
    let found: Option<String> = sqlx::query_scalar(
        "SELECT instance_id FROM instances WHERE instance_id=$1 AND tenant_id=$2",
    )
    .bind(instance)
    .bind(tenant)
    .fetch_optional(db)
    .await
    .map_err(storage)?;
    found.map(|_| ()).ok_or(InputError::NotFound)
}

/// Authorize every root inside the caller's read-only snapshot, in one round
/// trip and without row locks. Authorization failure never becomes a
/// partial/empty result.
async fn discovery_roots(
    db: &mut PgConnection,
    tenant: &str,
    instances: &[String],
) -> InputResult<Vec<String>> {
    let expected: std::collections::BTreeSet<_> = instances.iter().collect();
    let ids: Vec<String> = sqlx::query_scalar("SELECT instance_id FROM instances WHERE tenant_id=$1 AND instance_id=ANY($2) ORDER BY instance_id COLLATE \"C\"")
        .bind(tenant).bind(instances).fetch_all(db).await.map_err(storage)?;
    if ids.len() != expected.len() {
        return Err(InputError::NotFound);
    }
    Ok(ids)
}

async fn authority(db: &mut PgConnection, owner: &InputAuthority) -> InputResult<()> {
    if root(db, owner.tenant_id(), owner.instance_id()).await? != "running" {
        return Err(InputError::Inactive);
    }
    match owner {
        InputAuthority::Root { instance_id, .. } => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM invocation_root_leases WHERE instance_id=$1)",
            )
            .bind(instance_id)
            .fetch_one(db)
            .await
            .map_err(storage)?;
            if exists {
                Err(InputError::FenceRejected)
            } else {
                Ok(())
            }
        }
        InputAuthority::LeasedRoot(lease) => crate::invocations::check_lease(db, lease, true)
            .await
            .map_err(fence_error),
        InputAuthority::Invocation(fence) => {
            crate::invocations::check_lease(db, &fence.lease, true)
                .await
                .map_err(fence_error)?;
            if crate::invocations::check_attempt(db, fence)
                .await
                .map_err(fence_error)?
                != AttemptState::Active
            {
                return Err(InputError::FenceRejected);
            }
            Ok(())
        }
    }
}

fn fence_error(error: runtara_core::persistence::invocations::InvocationFenceError) -> InputError {
    match error {
        runtara_core::persistence::invocations::InvocationFenceError::Storage(message) => {
            InputError::Storage(message)
        }
        _ => InputError::FenceRejected,
    }
}

async fn load(
    db: &mut PgConnection,
    instance: &str,
    request: &str,
) -> InputResult<Option<InputRequest>> {
    sqlx::query_as::<_, RequestRow>(
        "SELECT * FROM instance_input_requests WHERE instance_id=$1 AND request_id=$2",
    )
    .bind(instance)
    .bind(request)
    .fetch_optional(db)
    .await
    .map_err(storage)?
    .map(RequestRow::record)
    .transpose()
}

async fn now(db: &mut PgConnection) -> InputResult<DateTime<Utc>> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(db)
        .await
        .map_err(storage)
}

async fn replay(
    db: &mut PgConnection,
    tenant: &str,
    instance: &str,
    request: &str,
    operation: &str,
    identity: InputReplayIdentity<'_>,
) -> InputResult<Option<InputReceipt>> {
    let found = sqlx::query_as::<_, RequestRow>("SELECT * FROM instance_input_requests WHERE tenant_id=$1 AND instance_id=$2 AND operation_id=$3")
        .bind(tenant).bind(instance).bind(operation).fetch_optional(db).await.map_err(storage)?;
    if let Some(row) = found {
        if let InputState::Accepted { receipt } = row.record()?.state {
            if receipt.request_id != request || !identity.matches(&receipt) {
                return Err(InputError::OperationConflict);
            }
            return Ok(Some(receipt));
        }
        return Err(storage("operation points at an unanswered input"));
    }
    Ok(None)
}

async fn owner_live(db: &mut PgConnection, request: &InputRequest) -> InputResult<bool> {
    if request.invocation_path.is_empty() {
        return Ok(true);
    }
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM invocation_attempts WHERE instance_id=$1 AND invocation_path=$2 ORDER BY generation DESC LIMIT 1")
        .bind(&request.instance_id).bind(&request.invocation_path).fetch_optional(db).await.map_err(storage)?;
    Ok(state.as_deref() == Some("active"))
}

// Used by both recovery candidate selection and the locked scheduling helper.
// All writers of the instance, ownership, park and request hold the root lock.
const WAKE_ELIGIBLE: &str = "i.status='suspended' AND i.termination_reason='waiting_signal' AND r.state='accepted' AND r.signal_id=ANY(p.signal_ids) AND (r.invocation_path='' OR (SELECT a.state FROM invocation_attempts a WHERE a.instance_id=r.instance_id AND a.invocation_path=r.invocation_path ORDER BY generation DESC LIMIT 1)='active')";

/// Schedule once per park, under the caller's root lock. Fresh parks inspect all
/// retained accepted responses, including ones whose earlier wake was handled.
pub(crate) async fn schedule_accepted(
    db: &mut PgConnection,
    instance: &str,
    all_accepted: bool,
) -> Result<bool, sqlx::Error> {
    let candidates: Vec<(String, bool)> = sqlx::query_as(&format!(
        "SELECT r.request_id, p.wake_scheduled FROM instances i
         JOIN instance_input_parks p USING(instance_id)
         JOIN instance_input_requests r USING(instance_id)
         WHERE i.instance_id=$1 AND ($2 OR r.wake_pending) AND {WAKE_ELIGIBLE}"
    ))
    .bind(instance)
    .bind(all_accepted)
    .fetch_all(&mut *db)
    .await?;
    let Some((_, scheduled)) = candidates.first() else {
        return Ok(false);
    };
    if !scheduled {
        sqlx::query("UPDATE instances SET sleep_until=LEAST(sleep_until, clock_timestamp()), wake_reason='custom_signal' WHERE instance_id=$1")
            .bind(instance).execute(&mut *db).await?;
        sqlx::query("UPDATE instance_input_parks SET wake_scheduled=true WHERE instance_id=$1")
            .bind(instance)
            .execute(&mut *db)
            .await?;
    }
    let ids: Vec<_> = candidates.into_iter().map(|(id, _)| id).collect();
    sqlx::query("UPDATE instance_input_requests SET wake_pending=false WHERE instance_id=$1 AND request_id=ANY($2)")
        .bind(instance).bind(&ids).execute(db).await?;
    Ok(true)
}

/// Called under the root lock by every terminal transition.
pub(crate) async fn close_roots(
    db: &mut PgConnection,
    instances: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE instances SET sleep_until=NULL, wake_reason=NULL WHERE instance_id=ANY($1)",
    )
    .bind(instances)
    .execute(&mut *db)
    .await?;
    sqlx::query("DELETE FROM instance_input_parks WHERE instance_id=ANY($1)")
        .bind(instances)
        .execute(&mut *db)
        .await?;
    sqlx::query("UPDATE instance_input_requests SET state=CASE WHEN state='open' THEN 'closed' ELSE state END, closure_reason=CASE WHEN state='open' THEN 'instance_terminated' ELSE closure_reason END, closed_at=CASE WHEN state='open' THEN clock_timestamp() ELSE closed_at END, wake_pending=false WHERE instance_id=ANY($1) AND (state='open' OR wake_pending)")
        .bind(instances).execute(db).await?;
    Ok(())
}

/// Invocation transitions retain answered receipts but invalidate further input.
pub(crate) async fn close_invocation(
    db: &mut PgConnection,
    instance: &str,
    path: &str,
    reason: InputClosure,
) -> Result<(), sqlx::Error> {
    let family: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE family(path) AS (
            SELECT $2::text UNION
            SELECT a.invocation_path FROM invocation_attempts a JOIN family f ON a.parent_path=f.path
            WHERE a.instance_id=$1
         ) SELECT path FROM family"
    ).bind(instance).bind(path).fetch_all(&mut *db).await?;
    // The ancestor transition also fences descendants with no input yet; they
    // cannot register a new request after the cancellation/settlement commits.
    sqlx::query("UPDATE invocation_attempts SET state='cancelled' WHERE instance_id=$1 AND invocation_path=ANY($2) AND invocation_path<>$3 AND state='active'")
        .bind(instance).bind(&family).bind(path).execute(&mut *db).await?;
    let reason = serde_json::to_value(reason).expect("closure serializes");
    sqlx::query("UPDATE instance_input_requests SET state=CASE WHEN state='open' THEN 'closed' ELSE state END, closure_reason=CASE WHEN state='open' THEN $3 ELSE closure_reason END, closed_at=CASE WHEN state='open' THEN clock_timestamp() ELSE closed_at END, wake_pending=false WHERE instance_id=$1 AND invocation_path=ANY($2) AND (state='open' OR wake_pending)")
        .bind(instance).bind(&family).bind(reason.as_str().unwrap()).execute(db).await?;
    Ok(())
}

impl PostgresPersistence {
    async fn input_transaction(&self) -> InputResult<Transaction<'static, Postgres>> {
        self.pool.begin().await.map_err(storage)
    }

    /// One consistent snapshot for count, page and eligibility, without locks.
    async fn read_transaction(&self) -> InputResult<Transaction<'static, Postgres>> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        Ok(tx)
    }
}

#[async_trait]
impl InputRequests for PostgresPersistence {
    async fn input_clock(&self) -> InputResult<DateTime<Utc>> {
        let mut db = self.pool.acquire().await.map_err(storage)?;
        now(&mut db).await
    }

    async fn reconcile_input_wakes(&self, limit: u32) -> InputResult<u64> {
        let mut tx = self.input_transaction().await?;
        // Lock roots first. Skip unrelated running/paused intents so they cannot
        // starve eligible recovery work at the front of a bounded batch.
        let roots: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT i.instance_id FROM instances i WHERE EXISTS (
                SELECT 1 FROM instance_input_parks p
                JOIN instance_input_requests r USING(instance_id)
                WHERE p.instance_id=i.instance_id AND r.wake_pending AND {WAKE_ELIGIBLE}
             ) ORDER BY i.instance_id LIMIT $1 FOR UPDATE OF i SKIP LOCKED"
        ))
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;
        let mut count = 0;
        for id in roots {
            count += u64::from(
                schedule_accepted(&mut tx, &id, false)
                    .await
                    .map_err(storage)?,
            );
        }
        tx.commit().await.map_err(storage)?;
        Ok(count)
    }

    async fn register_input(
        &self,
        owner: &InputAuthority,
        spec: &InputRequestSpec,
    ) -> InputResult<InputRequest> {
        spec.validate()?;
        let mut tx = self.input_transaction().await?;
        authority(&mut tx, owner).await?;
        let id = spec.request_id();
        let fence = match owner {
            InputAuthority::Invocation(fence) => {
                Some(serde_json::to_value(fence).map_err(storage)?)
            }
            _ => None,
        };
        if let Some(existing) = load(&mut tx, owner.instance_id(), &id).await? {
            // Identity only: the first registration's metadata and deadline win.
            if existing.spec.signal_id != spec.signal_id
                || existing.invocation_path != owner.invocation_path()
            {
                return Err(InputError::IdentityConflict);
            }
            sqlx::query("UPDATE instance_input_requests SET fence=$3 WHERE instance_id=$1 AND request_id=$2")
                .bind(owner.instance_id()).bind(&id).bind(fence).execute(&mut *tx).await.map_err(storage)?;
        } else {
            let raw: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pending_checkpoint_signals WHERE instance_id=$1 AND checkpoint_id=$2)")
                .bind(owner.instance_id()).bind(&spec.signal_id).fetch_one(&mut *tx).await.map_err(storage)?;
            if raw {
                return Err(InputError::RawSignalConflict);
            }
            let at = now(&mut tx).await?;
            let expired = spec.deadline.is_some_and(|deadline| deadline <= at);
            sqlx::query("INSERT INTO instance_input_requests (instance_id,tenant_id,request_id,signal_id,invocation_path,fence,spec,created_at,deadline,state,closure_reason,closed_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
                .bind(owner.instance_id()).bind(owner.tenant_id()).bind(&id).bind(&spec.signal_id).bind(owner.invocation_path()).bind(fence)
                .bind(serde_json::to_string(spec).map_err(storage)?).bind(at).bind(spec.deadline)
                .bind(if expired { "closed" } else { "open" }).bind(expired.then_some("expired")).bind(expired.then_some(at))
                .execute(&mut *tx).await.map_err(storage)?;
        }
        let record = load(&mut tx, owner.instance_id(), &id)
            .await?
            .ok_or(InputError::NotFound)?;
        tx.commit().await.map_err(storage)?;
        Ok(record)
    }

    async fn get_input(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
    ) -> InputResult<InputRequest> {
        let mut tx = self.read_transaction().await?;
        root_snapshot(&mut tx, tenant, instance).await?;
        let record = load(&mut tx, instance, request)
            .await?
            .ok_or(InputError::NotFound)?;
        tx.commit().await.map_err(storage)?;
        Ok(record)
    }

    async fn replay_input(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
        operation: &str,
        identity: InputReplayIdentity<'_>,
    ) -> InputResult<Option<InputReceipt>> {
        validate_operation_id(operation)?;
        // Pre-check only: accept_input repeats replay under the root lock.
        let mut tx = self.read_transaction().await?;
        root_snapshot(&mut tx, tenant, instance).await?;
        let receipt = replay(&mut tx, tenant, instance, request, operation, identity).await?;
        tx.commit().await.map_err(storage)?;
        Ok(receipt)
    }

    async fn poll_input(&self, owner: &InputAuthority, request: &str) -> InputResult<InputRequest> {
        // Conditional expiry is also the guest's authoritative read. It returns
        // open before the deadline and retains a response that won acceptance.
        self.close_input(owner, request, InputClosure::Expired)
            .await
    }

    async fn accept_input(
        &self,
        tenant: &str,
        instance: &str,
        response: &ValidatedInputResponse,
    ) -> InputResult<InputReceipt> {
        let mut tx = self.input_transaction().await?;
        let status = root(&mut tx, tenant, instance).await?;
        let id = response.spec().request_id();
        if let Some(receipt) = replay(
            &mut tx,
            tenant,
            instance,
            &id,
            response.operation_id(),
            response.replay_identity(),
        )
        .await?
        {
            tx.commit().await.map_err(storage)?;
            return Ok(receipt);
        }
        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(InputError::Inactive);
        }
        let request = load(&mut tx, instance, &id)
            .await?
            .ok_or(InputError::NotFound)?;
        if request.spec != *response.spec() {
            return Err(InputError::IdentityConflict);
        }
        if matches!(request.state, InputState::Accepted { .. }) {
            return Err(InputError::AlreadyAnswered);
        }
        let at = now(&mut tx).await?;
        if !request.open_at(at) || !owner_live(&mut tx, &request).await? {
            return Err(InputError::Inactive);
        }
        let receipt = InputReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            operation_id: response.operation_id().into(),
            request_id: id.clone(),
            accepted_at: at,
            payload: response.payload().to_vec(),
            acceptance_context: response.acceptance_context().map(<[u8]>::to_vec),
        };
        // The root lock already serializes writers; the state guard keeps a
        // future lock-free writer from turning closed into accepted.
        let updated = sqlx::query("UPDATE instance_input_requests SET state='accepted', receipt_id=$3::uuid, operation_id=$4, accepted_payload=$5, accepted_at=$6, acceptance_context=$7, wake_pending=true WHERE instance_id=$1 AND request_id=$2 AND state='open'")
            .bind(instance).bind(&id).bind(&receipt.receipt_id).bind(&receipt.operation_id).bind(&receipt.payload).bind(at).bind(&receipt.acceptance_context)
            .execute(&mut *tx).await.map_err(storage)?;
        if updated.rows_affected() != 1 {
            return Err(InputError::Inactive);
        }
        schedule_accepted(&mut tx, instance, false)
            .await
            .map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(receipt)
    }

    async fn close_input(
        &self,
        owner: &InputAuthority,
        request: &str,
        reason: InputClosure,
    ) -> InputResult<InputRequest> {
        let mut tx = self.input_transaction().await?;
        authority(&mut tx, owner).await?;
        let mut record = load(&mut tx, owner.instance_id(), request)
            .await?
            .ok_or(InputError::NotFound)?;
        if record.invocation_path != owner.invocation_path() {
            return Err(InputError::FenceRejected);
        }
        let at = now(&mut tx).await?;
        if reason != InputClosure::Expired
            || record.spec.deadline.is_some_and(|deadline| deadline <= at)
        {
            let reason = serde_json::to_value(reason).map_err(storage)?;
            sqlx::query("UPDATE instance_input_requests SET state=CASE WHEN state='open' THEN 'closed' ELSE state END, closure_reason=CASE WHEN state='open' THEN $3 ELSE closure_reason END, closed_at=CASE WHEN state='open' THEN $4 ELSE closed_at END, wake_pending=CASE WHEN state='open' THEN false ELSE wake_pending END WHERE instance_id=$1 AND request_id=$2")
                .bind(owner.instance_id()).bind(request).bind(reason.as_str().unwrap()).bind(at).execute(&mut *tx).await.map_err(storage)?;
            record = load(&mut tx, owner.instance_id(), request)
                .await?
                .ok_or(InputError::NotFound)?;
        }
        tx.commit().await.map_err(storage)?;
        Ok(record)
    }

    async fn list_inputs(
        &self,
        tenant: &str,
        instances: &[String],
        offset: u64,
        limit: u32,
    ) -> InputResult<InputRequestPage> {
        // A repeatable-read snapshot makes authorization, count, page and
        // invocation state consistent without blocking any writer.
        let mut tx = self.read_transaction().await?;
        let ids = discovery_roots(&mut tx, tenant, instances).await?;
        let at = now(&mut tx).await?;
        let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM instance_input_requests r JOIN instances i USING(instance_id) WHERE {ACTIONABLE_FILTER}"))
            .bind(tenant).bind(&ids).bind(at).fetch_one(&mut *tx).await.map_err(storage)?;
        let rows = sqlx::query_as::<_, RequestRow>(&format!("SELECT r.* FROM instance_input_requests r JOIN instances i USING(instance_id) WHERE {ACTIONABLE_FILTER} ORDER BY r.created_at, r.request_id COLLATE \"C\", r.instance_id COLLATE \"C\" LIMIT $4 OFFSET $5"))
            .bind(tenant).bind(&ids).bind(at).bind(i64::from(limit)).bind(i64::try_from(offset).unwrap_or(i64::MAX))
            .fetch_all(&mut *tx).await.map_err(storage)?;
        let requests = rows
            .into_iter()
            .map(RequestRow::record)
            .collect::<InputResult<Vec<_>>>()?;
        tx.commit().await.map_err(storage)?;
        Ok(InputRequestPage {
            requests,
            total_count: total as u64,
        })
    }

    async fn instances_with_open_inputs(
        &self,
        tenant: &str,
        instances: &[String],
    ) -> InputResult<std::collections::BTreeSet<String>> {
        let mut tx = self.read_transaction().await?;
        let ids = discovery_roots(&mut tx, tenant, instances).await?;
        let at = now(&mut tx).await?;
        let found: Vec<String> = sqlx::query_scalar(&format!("SELECT DISTINCT r.instance_id FROM instance_input_requests r JOIN instances i USING(instance_id) WHERE {ACTIONABLE_FILTER}"))
            .bind(tenant).bind(&ids).bind(at).fetch_all(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(found.into_iter().collect())
    }
}
