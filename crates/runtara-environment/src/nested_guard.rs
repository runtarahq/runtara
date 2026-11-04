// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A bound on guard transactions that take more connections from their pool
//! while they hold one.
//!
//! Settling an observed exit and recovering a run each lock their launch and
//! registration rows in a guard transaction, then apply the lifecycle change
//! through Core, which acquires its own connections from the same pool. With
//! as many of them at once as the pool has connections (a burst of lost runs),
//! every connection was a guard waiting for a second one: nothing moved until
//! acquires timed out, and recovery took minutes. Admitted here, at most a
//! quarter of the pool holds guards, so the connections they wait for are
//! there.

use std::sync::OnceLock;

use tokio::sync::{Semaphore, SemaphorePermit};

static GUARDS: OnceLock<Semaphore> = OnceLock::new();

/// How many guards may hold connections at once for a pool of `max`.
fn permits(max: u32) -> usize {
    usize::try_from(max / 4).unwrap_or(1).max(1)
}

/// Waits, holding no connection, until a guard transaction on `pool` may
/// begin; the guard ends when the permit is dropped. Sized from the first
/// pool seen: a process has one runtime pool.
pub(crate) async fn admit(pool: &sqlx::PgPool) -> SemaphorePermit<'static> {
    GUARDS
        .get_or_init(|| Semaphore::new(permits(pool.options().get_max_connections())))
        .acquire()
        .await
        .expect("nested guard semaphore is never closed")
}

#[cfg(test)]
mod tests {
    use super::permits;

    #[test]
    fn a_quarter_of_the_pool_and_never_none() {
        assert_eq!(permits(32), 8);
        assert_eq!(permits(10), 2);
        assert_eq!(permits(3), 1);
        assert_eq!(permits(0), 1);
    }
}
