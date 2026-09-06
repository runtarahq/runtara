//! Atomic invocation transitions. Lock the tenant-owned root row first in every
//! transaction, including cancellation; root lifecycle UPDATEs share that lock.
use crate::PostgresPersistence;
use runtara_core::persistence::invocations::*;
use sqlx::{PgConnection, Postgres, Transaction};

fn denied(reason: FenceRejection) -> InvocationFenceError {
    InvocationFenceError::Rejected(reason)
}
fn storage(error: sqlx::Error) -> InvocationFenceError {
    InvocationFenceError::Storage(error.to_string())
}

#[derive(sqlx::FromRow)]
struct LeaseRow {
    owner: String,
    epoch: i64,
    active: bool,
}
#[derive(sqlx::FromRow)]
struct AttemptRow {
    owner: String,
    lease_epoch: i64,
    invocation_path: String,
    generation: i64,
    start_id: String,
    state: String,
}
impl AttemptRow {
    fn record(self, tenant: &str, instance: &str) -> FenceResult<InvocationAttempt> {
        let state = match self.state.as_str() {
            "active" => AttemptState::Active,
            "settled" => AttemptState::Settled,
            "cancelled" => AttemptState::Cancelled,
            _ => {
                return Err(InvocationFenceError::Storage(
                    "invalid persisted attempt state".into(),
                ));
            }
        };
        Ok(InvocationAttempt {
            fence: AttemptFence {
                lease: InvocationLease {
                    tenant_id: tenant.into(),
                    instance_id: instance.into(),
                    owner: self.owner,
                    epoch: self.lease_epoch,
                },
                path: self.invocation_path,
                generation: self.generation,
                start_id: self.start_id,
            },
            state,
        })
    }
}
impl PostgresPersistence {
    async fn invocation_transaction(
        &self,
        tenant: &str,
        instance: &str,
        running: bool,
    ) -> FenceResult<Transaction<'static, Postgres>> {
        validate_identity(tenant)?;
        validate_identity(instance)?;
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status::text FROM instances WHERE instance_id=$1 AND tenant_id=$2 FOR UPDATE",
        )
        .bind(instance)
        .bind(tenant)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        let status = status.ok_or_else(|| denied(FenceRejection::UnknownRoot))?;
        if running && status != "running" {
            return Err(denied(FenceRejection::InactiveRoot));
        }
        Ok(tx)
    }
}
async fn load_lease(db: &mut PgConnection, instance: &str) -> FenceResult<Option<LeaseRow>> {
    sqlx::query_as("SELECT owner, epoch, active FROM invocation_root_leases WHERE instance_id=$1")
        .bind(instance)
        .fetch_optional(db)
        .await
        .map_err(storage)
}
async fn check_lease(
    db: &mut PgConnection,
    token: &InvocationLease,
    active: bool,
) -> FenceResult<()> {
    match load_lease(db, &token.instance_id).await? {
        Some(row)
            if row.owner == token.owner && row.epoch == token.epoch && (!active || row.active) =>
        {
            Ok(())
        }
        _ => Err(denied(FenceRejection::LeaseMismatch)),
    }
}
async fn latest(
    db: &mut PgConnection,
    lease: &InvocationLease,
    path: &str,
) -> FenceResult<Option<InvocationAttempt>> {
    let row: Option<AttemptRow> = sqlx::query_as("SELECT owner, lease_epoch, invocation_path, generation, start_id, state FROM invocation_attempts WHERE instance_id=$1 AND invocation_path=$2 ORDER BY generation DESC LIMIT 1")
        .bind(&lease.instance_id).bind(path).fetch_optional(db).await.map_err(storage)?;
    row.map(|r| r.record(&lease.tenant_id, &lease.instance_id))
        .transpose()
}
async fn check_attempt(db: &mut PgConnection, token: &AttemptFence) -> FenceResult<AttemptState> {
    let current = latest(db, &token.lease, &token.path)
        .await?
        .ok_or_else(|| denied(FenceRejection::AttemptMismatch))?;
    if current.fence != *token {
        return Err(denied(FenceRejection::AttemptMismatch));
    }
    Ok(current.state)
}
async fn checkpoint(
    db: &mut PgConnection,
    token: &AttemptFence,
    write: &InvocationCheckpoint,
) -> FenceResult<InvocationCheckpointResult> {
    let existing: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT state FROM checkpoints WHERE instance_id=$1 AND checkpoint_id=$2",
    )
    .bind(&token.lease.instance_id)
    .bind(&write.checkpoint_id)
    .fetch_optional(&mut *db)
    .await
    .map_err(storage)?;
    if let Some(state) = existing {
        return Ok(InvocationCheckpointResult { found: true, state });
    }
    if !write.state.is_empty() {
        let inserted = sqlx::query("INSERT INTO checkpoints (instance_id,checkpoint_id,state,created_at) VALUES ($1,$2,$3,NOW()) ON CONFLICT (instance_id,checkpoint_id) DO NOTHING")
            .bind(&token.lease.instance_id).bind(&write.checkpoint_id).bind(&write.state).execute(&mut *db).await.map_err(storage)?.rows_affected();
        if inserted == 0 {
            let state = sqlx::query_scalar(
                "SELECT state FROM checkpoints WHERE instance_id=$1 AND checkpoint_id=$2",
            )
            .bind(&token.lease.instance_id)
            .bind(&write.checkpoint_id)
            .fetch_one(&mut *db)
            .await
            .map_err(storage)?;
            return Ok(InvocationCheckpointResult { found: true, state });
        }
        sqlx::query("UPDATE instances SET checkpoint_id=$2 WHERE instance_id=$1")
            .bind(&token.lease.instance_id)
            .bind(&write.checkpoint_id)
            .execute(&mut *db)
            .await
            .map_err(storage)?;
    }
    Ok(InvocationCheckpointResult {
        found: false,
        state: vec![],
    })
}

#[async_trait::async_trait]
impl InvocationFences for PostgresPersistence {
    async fn get_invocation_lease(
        &self,
        tenant: &str,
        instance: &str,
    ) -> FenceResult<Option<InvocationLeaseState>> {
        let mut tx = self.invocation_transaction(tenant, instance, false).await?;
        let row = load_lease(&mut tx, instance).await?;
        tx.commit().await.map_err(storage)?;
        Ok(row.map(|row| InvocationLeaseState {
            lease: InvocationLease {
                tenant_id: tenant.into(),
                instance_id: instance.into(),
                owner: row.owner,
                epoch: row.epoch,
            },
            active: row.active,
        }))
    }

    async fn claim_invocation_lease(
        &self,
        tenant: &str,
        instance: &str,
        owner: &str,
        expected_epoch: Option<i64>,
    ) -> FenceResult<InvocationLease> {
        validate_identity(owner)?;
        if expected_epoch.is_some_and(|epoch| epoch <= 0 || epoch == i64::MAX) {
            return Err(denied(FenceRejection::InvalidIdentity));
        }
        let mut tx = self.invocation_transaction(tenant, instance, true).await?;
        let next = expected_epoch.unwrap_or(0) + 1;
        if let Some(current) = load_lease(&mut tx, instance).await? {
            if current.owner == owner && current.epoch == next && current.active {
                tx.commit().await.map_err(storage)?;
                return Ok(InvocationLease {
                    tenant_id: tenant.into(),
                    instance_id: instance.into(),
                    owner: owner.into(),
                    epoch: next,
                });
            }
            if current.active || Some(current.epoch) != expected_epoch {
                return Err(denied(FenceRejection::LeaseMismatch));
            }
        } else if expected_epoch.is_some() {
            return Err(denied(FenceRejection::LeaseMismatch));
        }
        sqlx::query("INSERT INTO invocation_root_leases (instance_id,owner,epoch,active) VALUES ($1,$2,$3,true) ON CONFLICT (instance_id) DO UPDATE SET owner=EXCLUDED.owner,epoch=EXCLUDED.epoch,active=true")
            .bind(instance).bind(owner).bind(next).execute(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(InvocationLease {
            tenant_id: tenant.into(),
            instance_id: instance.into(),
            owner: owner.into(),
            epoch: next,
        })
    }
    async fn revoke_invocation_lease(&self, token: &InvocationLease) -> FenceResult<()> {
        let mut tx = self
            .invocation_transaction(&token.tenant_id, &token.instance_id, false)
            .await?;
        check_lease(&mut tx, token, false).await?;
        sqlx::query("UPDATE invocation_root_leases SET active=false WHERE instance_id=$1")
            .bind(&token.instance_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        tx.commit().await.map_err(storage)
    }
    async fn begin_invocation_attempt(
        &self,
        token: &InvocationLease,
        path: &str,
        start_id: &str,
    ) -> FenceResult<InvocationAttempt> {
        validate_identity(path)?;
        validate_identity(start_id)?;
        let mut tx = self
            .invocation_transaction(&token.tenant_id, &token.instance_id, true)
            .await?;
        check_lease(&mut tx, token, true).await?;
        let previous: Option<AttemptRow> = sqlx::query_as("SELECT owner,lease_epoch,invocation_path,generation,start_id,state FROM invocation_attempts WHERE instance_id=$1 AND lease_epoch=$2 AND start_id=$3")
            .bind(&token.instance_id).bind(token.epoch).bind(start_id).fetch_optional(&mut *tx).await.map_err(storage)?;
        if let Some(previous) = previous {
            if previous.invocation_path != path {
                return Err(denied(FenceRejection::AttemptMismatch));
            }
            let record = previous.record(&token.tenant_id, &token.instance_id)?;
            tx.commit().await.map_err(storage)?;
            return Ok(record);
        }
        if let Some(previous) = latest(&mut tx, token, path).await? {
            if previous.state == AttemptState::Cancelled {
                tx.commit().await.map_err(storage)?;
                return Ok(previous);
            }
            if previous.state == AttemptState::Active && previous.fence.lease == *token {
                return Err(denied(FenceRejection::Busy));
            }
        }
        let generation: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(generation),0) FROM invocation_attempts WHERE instance_id=$1",
        )
        .bind(&token.instance_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        let generation = generation
            .checked_add(1)
            .ok_or_else(|| denied(FenceRejection::InvalidIdentity))?;
        sqlx::query("INSERT INTO invocation_attempts (instance_id,generation,lease_epoch,owner,invocation_path,start_id,state) VALUES ($1,$2,$3,$4,$5,$6,'active')")
            .bind(&token.instance_id).bind(generation).bind(token.epoch).bind(&token.owner).bind(path).bind(start_id).execute(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(InvocationAttempt {
            fence: AttemptFence {
                lease: token.clone(),
                path: path.into(),
                start_id: start_id.into(),
                generation,
            },
            state: AttemptState::Active,
        })
    }
    async fn cancel_invocation_attempt(&self, token: &AttemptFence) -> FenceResult<AttemptState> {
        let mut tx = self
            .invocation_transaction(&token.lease.tenant_id, &token.lease.instance_id, false)
            .await?;
        let mut state = check_attempt(&mut tx, token).await?;
        if state == AttemptState::Active {
            sqlx::query("UPDATE invocation_attempts SET state='cancelled' WHERE instance_id=$1 AND generation=$2").bind(&token.lease.instance_id).bind(token.generation).execute(&mut *tx).await.map_err(storage)?;
            state = AttemptState::Cancelled;
        }
        tx.commit().await.map_err(storage)?;
        Ok(state)
    }
    async fn settle_invocation_attempt(
        &self,
        token: &AttemptFence,
        write: Option<&InvocationCheckpoint>,
    ) -> FenceResult<InvocationSettlement> {
        if let Some(write) = write {
            validate_identity(&write.checkpoint_id)?;
        }
        let mut tx = self
            .invocation_transaction(&token.lease.tenant_id, &token.lease.instance_id, true)
            .await?;
        check_lease(&mut tx, &token.lease, true).await?;
        let mut state = check_attempt(&mut tx, token).await?;
        let mut committed = None;
        if state == AttemptState::Active {
            if let Some(write) = write {
                committed = Some(checkpoint(&mut tx, token, write).await?);
            }
            sqlx::query("UPDATE invocation_attempts SET state='settled' WHERE instance_id=$1 AND generation=$2").bind(&token.lease.instance_id).bind(token.generation).execute(&mut *tx).await.map_err(storage)?;
            state = AttemptState::Settled;
        }
        tx.commit().await.map_err(storage)?;
        Ok(InvocationSettlement {
            state,
            checkpoint: committed,
        })
    }
    async fn invocation_checkpoint(
        &self,
        token: &AttemptFence,
        write: &InvocationCheckpoint,
    ) -> FenceResult<InvocationCheckpointResult> {
        validate_identity(&write.checkpoint_id)?;
        let mut tx = self
            .invocation_transaction(&token.lease.tenant_id, &token.lease.instance_id, true)
            .await?;
        check_lease(&mut tx, &token.lease, true).await?;
        match check_attempt(&mut tx, token).await? {
            AttemptState::Cancelled => return Err(denied(FenceRejection::Cancelled)),
            AttemptState::Settled => return Err(denied(FenceRejection::Settled)),
            AttemptState::Active => {}
        }
        let result = checkpoint(&mut tx, token, write).await?;
        tx.commit().await.map_err(storage)?;
        Ok(result)
    }
}
