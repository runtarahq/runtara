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
