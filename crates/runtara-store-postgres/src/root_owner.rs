// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Root execution ownership: the check an execution's write makes against
//! the instance's root lease.
//!
//! Every caller locks the instance row in an earlier statement of the same
//! transaction and only then reads the lease. Claims and revocations change
//! the lease under that same row lock, so a write that waited on it reads the
//! committed replacement rather than the snapshot it started with.
use crate::rows::DbResult;
use runtara_core::error::CoreError;
use runtara_core::persistence::ExecutionWriter;
use runtara_core::persistence::invocations::InvocationLease;
use sqlx::PgConnection;

/// The stored root lease of one instance.
#[derive(sqlx::FromRow)]
pub(crate) struct StoredLease {
    owner: String,
    epoch: i64,
    pub active: bool,
    pub parked: bool,
}

impl StoredLease {
    /// Whether this row is exactly `lease`, active or not.
    pub fn is(&self, lease: &InvocationLease) -> bool {
        self.owner == lease.owner && self.epoch == lease.epoch
    }
}

/// Read the root lease. The caller must already hold the instance row lock.
pub(crate) async fn load(
    db: &mut PgConnection,
    instance_id: &str,
) -> Result<Option<StoredLease>, CoreError> {
    sqlx::query_as(
        "SELECT owner, epoch, active, parked FROM invocation_root_leases WHERE instance_id = $1",
    )
    .bind(instance_id)
    .fetch_optional(db)
    .await
    .db()
}

/// Whether `lease` is the instance's active root lease. The caller must
/// already hold the instance row lock.
pub(crate) async fn holds(
    db: &mut PgConnection,
    lease: &InvocationLease,
) -> Result<bool, CoreError> {
    Ok(load(db, &lease.instance_id)
        .await?
        .is_some_and(|stored| stored.active && stored.is(lease)))
}

/// Whether `owner` may write the instance's guest state, given its stored
/// lease: see [`ExecutionWriter`].
fn admits(stored: Option<&StoredLease>, owner: ExecutionWriter<'_>) -> bool {
    match owner {
        Some(lease) => stored.is_some_and(|stored| stored.active && stored.is(lease)),
        None => !stored.is_some_and(|stored| stored.active),
    }
}

fn superseded(instance_id: &str) -> CoreError {
    CoreError::Superseded {
        instance_id: instance_id.to_string(),
    }
}

/// Fence a guest write that also updates the instance row (a checkpoint and
/// its pointer). Lock the row first, which serializes with every claim and
/// revocation (both change the lease under it), then read the lease in a
/// later statement so it is never a stale snapshot.
pub(crate) async fn admit_row_writer(
    db: &mut PgConnection,
    instance_id: &str,
    owner: ExecutionWriter<'_>,
) -> Result<(), CoreError> {
    let locked: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM instances WHERE instance_id = $1 FOR NO KEY UPDATE")
            .bind(instance_id)
            .fetch_optional(&mut *db)
            .await
            .db()?;
    if locked.is_none() {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }
    if admits(load(db, instance_id).await?.as_ref(), owner) {
        Ok(())
    } else {
        Err(superseded(instance_id))
    }
}

/// The `fence` CTE of a single-statement guest write that does not update
/// the instance row. It yields the instance id once when the writer is
/// admitted and nothing otherwise; the write selects from it.
///
/// An owned writer locks the instance row (`FOR KEY SHARE`, which status
/// updates do not conflict with) and then its exact active lease row
/// (`FOR SHARE`), in that order, the same order every lifecycle writer
/// takes them, so the two never deadlock. A revocation or claim must update
/// the lease row, so it waits for this write, and a write that waited
/// re-checks the rows' committed versions. Bind `$1` to the instance id and,
/// when owned, `$owner_param` and the next parameter to the lease owner and
/// epoch ([`bind_owner`]).
///
/// An unowned writer (outside any launched execution) is admitted while no
/// execution holds the lease, checked against the instance row it locks.
pub(crate) fn fence(owner: ExecutionWriter<'_>, owner_param: usize, running_only: bool) -> String {
    match owner {
        Some(_) => format!(
            "SELECT i.instance_id FROM instances i \
             JOIN invocation_root_leases l ON l.instance_id = i.instance_id \
             WHERE i.instance_id = $1 AND l.owner = ${owner_param} AND l.epoch = ${} \
               AND l.active \
             FOR KEY SHARE OF i FOR SHARE OF l",
            owner_param + 1
        ),
        None => format!(
            "SELECT i.instance_id FROM instances i \
             WHERE i.instance_id = $1{} \
               AND NOT EXISTS (SELECT 1 FROM invocation_root_leases l \
                               WHERE l.instance_id = i.instance_id AND l.active) \
             FOR SHARE OF i",
            if running_only {
                " AND i.status = 'running'"
            } else {
                ""
            }
        ),
    }
}

/// Bind the owner parameters a [`fence`] expects, if any.
pub(crate) fn bind_owner<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    owner: ExecutionWriter<'q>,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match owner {
        Some(lease) => query.bind(&lease.owner).bind(lease.epoch),
        None => query,
    }
}

/// Why a fenced single-statement write wrote nothing: a missing instance,
/// or a writer the instance's lease does not admit.
pub(crate) async fn refusal(pool: &sqlx::PgPool, instance_id: &str) -> CoreError {
    match sqlx::query_scalar::<_, i32>("SELECT 1 FROM instances WHERE instance_id = $1")
        .bind(instance_id)
        .fetch_optional(pool)
        .await
    {
        Ok(None) => CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        },
        Ok(Some(_)) => superseded(instance_id),
        Err(error) => CoreError::PersistenceError {
            operation: "fenced write".into(),
            details: error.to_string(),
        },
    }
}
