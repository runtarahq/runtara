//! Atomic root lease transitions. Lock the tenant-owned root row first in every
//! transaction; root lifecycle UPDATEs share that lock.
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
/// A root's status and lease, read in one statement without locking. Used
/// only where the locked transaction would have committed without writing:
/// its lock was released before the caller saw the result, so a plain read
/// (a snapshot just before any in-flight writer commits) is equivalent.
async fn read_root(
    pool: &sqlx::PgPool,
    tenant: &str,
    instance: &str,
) -> FenceResult<(String, Option<LeaseRow>)> {
    validate_identity(tenant)?;
    validate_identity(instance)?;
    #[derive(sqlx::FromRow)]
    struct RootRow {
        status: String,
        owner: Option<String>,
        epoch: Option<i64>,
        active: Option<bool>,
    }
    let row: Option<RootRow> = sqlx::query_as(
        "SELECT i.status::text AS status, l.owner, l.epoch, l.active FROM instances i \
         LEFT JOIN invocation_root_leases l ON l.instance_id = i.instance_id \
         WHERE i.instance_id=$1 AND i.tenant_id=$2",
    )
    .bind(instance)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .map_err(storage)?;
    let row = row.ok_or_else(|| denied(FenceRejection::UnknownRoot))?;
    let status = row.status;
    let lease = match (row.owner, row.epoch, row.active) {
        (Some(owner), Some(epoch), Some(active)) => Some(LeaseRow {
            owner,
            epoch,
            active,
        }),
        _ => None,
    };
    Ok((status, lease))
}
async fn load_lease(db: &mut PgConnection, instance: &str) -> FenceResult<Option<LeaseRow>> {
    sqlx::query_as("SELECT owner, epoch, active FROM invocation_root_leases WHERE instance_id=$1")
        .bind(instance)
        .fetch_optional(db)
        .await
        .map_err(storage)
}
pub(crate) async fn check_lease(
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
#[async_trait::async_trait]
impl InvocationFences for PostgresPersistence {
    async fn get_invocation_lease(
        &self,
        tenant: &str,
        instance: &str,
    ) -> FenceResult<Option<InvocationLeaseState>> {
        let (_, row) = read_root(&self.pool, tenant, instance).await?;
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
        let next = expected_epoch.unwrap_or(0) + 1;
        // Already this owner's (a supervisor claimed it when it promoted the
        // run): nothing to write, so no transaction.
        if let (status, Some(current)) = read_root(&self.pool, tenant, instance).await?
            && status == "running"
            && current.owner == owner
            && current.epoch == next
            && current.active
        {
            return Ok(InvocationLease {
                tenant_id: tenant.into(),
                instance_id: instance.into(),
                owner: owner.into(),
                epoch: next,
            });
        }
        let mut tx = self.invocation_transaction(tenant, instance, true).await?;
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
        sqlx::query("INSERT INTO invocation_root_leases (instance_id,owner,epoch,active) VALUES ($1,$2,$3,true) ON CONFLICT (instance_id) DO UPDATE SET owner=EXCLUDED.owner,epoch=EXCLUDED.epoch,active=true,released_by=NULL")
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
        validate_identity(&token.tenant_id)?;
        validate_identity(&token.instance_id)?;
        // Already inactive (a terminal transition deactivates its root's
        // lease under the root's lock): every write presenting it is refused
        // already, so there is nothing to revoke and nothing is written.
        if let (_, Some(lease)) =
            read_root(&self.pool, &token.tenant_id, &token.instance_id).await?
            && lease.owner == token.owner
            && lease.epoch == token.epoch
            && !lease.active
        {
            return Ok(());
        }
        // One statement, still taking the root row's lock first: a write that
        // checked this lease under that lock commits before the revocation,
        // and none after it.
        let revoked = sqlx::query(
            "WITH root AS (SELECT instance_id FROM instances \
                 WHERE instance_id=$1 AND tenant_id=$2 FOR UPDATE) \
             UPDATE invocation_root_leases l SET active=false FROM root \
             WHERE l.instance_id=root.instance_id AND l.owner=$3 AND l.epoch=$4",
        )
        .bind(&token.instance_id)
        .bind(&token.tenant_id)
        .bind(&token.owner)
        .bind(token.epoch)
        .execute(&self.pool)
        .await
        .map_err(storage)?
        .rows_affected();
        if revoked == 1 {
            return Ok(());
        }
        // Say why: an unknown root, or another lease.
        read_root(&self.pool, &token.tenant_id, &token.instance_id).await?;
        Err(denied(FenceRejection::LeaseMismatch))
    }
}
