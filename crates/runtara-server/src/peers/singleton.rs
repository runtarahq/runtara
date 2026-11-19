// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A lease in Valkey for a job no two engines should run at once:
//! `runtara:singleton:<name>`, its value the holder's engine id.
//!
//! The holder renews it every third of its lease; an engine that stops
//! renewing loses it when it expires, and another takes it. A holder that
//! cannot reach Valkey takes itself to have lost it at once, so two engines
//! hold it at the same time only within one renewal. A job's correctness
//! still rests on its own guards in the database: the lease keeps the work
//! from being done twice, not from being done wrong.

use std::time::Duration;

use redis::aio::ConnectionManager;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::shutdown::ShutdownSignal;

/// The key of the lease named `name`.
pub fn key(name: &str) -> String {
    format!("runtara:singleton:{name}")
}

/// Takes the lease if it is free, renews it if `owner` holds it; 1 when
/// `owner` holds it after.
const TAKE_OR_RENEW: &str = r#"
local holder = redis.call('GET', KEYS[1])
if holder == false then
  redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
  return 1
end
if holder == ARGV[1] then
  redis.call('PEXPIRE', KEYS[1], ARGV[2])
  return 1
end
return 0
"#;

/// Removes the lease if `owner` holds it.
const RELEASE: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
"#;

/// One attempt to take or renew the lease `name` for `owner`.
pub async fn take_or_renew(
    manager: &ConnectionManager,
    name: &str,
    owner: &str,
    lease: Duration,
) -> Result<bool, redis::RedisError> {
    let mut conn = manager.clone();
    let held: i64 = redis::Script::new(TAKE_OR_RENEW)
        .key(key(name))
        .arg(owner)
        .arg(lease.as_millis() as u64)
        .invoke_async(&mut conn)
        .await?;
    Ok(held == 1)
}

/// Gives the lease `name` up, if `owner` holds it.
pub async fn release(
    manager: &ConnectionManager,
    name: &str,
    owner: &str,
) -> Result<(), redis::RedisError> {
    let mut conn = manager.clone();
    let _: i64 = redis::Script::new(RELEASE)
        .key(key(name))
        .arg(owner)
        .invoke_async(&mut conn)
        .await?;
    Ok(())
}

/// Whether this engine holds a lease, kept current by a task of its own.
#[derive(Clone)]
pub struct Singleton {
    name: String,
    held: watch::Receiver<bool>,
}

impl Singleton {
    /// Keeps trying to take the lease `name` for `owner`, and renewing it
    /// while held, until shutdown, when it is given up.
    pub fn start(
        manager: ConnectionManager,
        name: impl Into<String>,
        owner: impl Into<String>,
        lease: Duration,
        shutdown: ShutdownSignal,
    ) -> Self {
        let name = name.into();
        let owner = owner.into();
        let (tx, held) = watch::channel(false);
        let task_name = name.clone();
        runtara_store_postgres::tenant_scope::spawn(async move {
            let every = lease / 3;
            loop {
                let now_held = match take_or_renew(&manager, &task_name, &owner, lease).await {
                    Ok(held) => held,
                    Err(error) => {
                        warn!(lease = %task_name, error = %error, "could not renew a singleton lease");
                        false
                    }
                };
                let was_held = *tx.borrow();
                if now_held != was_held {
                    if now_held {
                        info!(lease = %task_name, "this engine took a singleton lease");
                    } else {
                        info!(lease = %task_name, "this engine lost a singleton lease");
                    }
                    let _ = tx.send(now_held);
                }
                tokio::select! {
                    _ = tokio::time::sleep(every) => {}
                    _ = shutdown.clone().wait() => {}
                }
                if shutdown.is_shutting_down() {
                    let _ = tx.send(false);
                    if let Err(error) = release(&manager, &task_name, &owner).await {
                        warn!(lease = %task_name, error = %error, "could not give a singleton lease up");
                    }
                    return;
                }
            }
        });
        Self { name, held }
    }

    /// The lease's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this engine holds it now.
    pub fn is_held(&self) -> bool {
        *self.held.borrow()
    }

    /// Waits until this engine holds the lease after not holding it: it
    /// returns at once the first time if it is held already. `false` once
    /// the lease is no longer kept (shutdown).
    pub async fn taken(&mut self) -> bool {
        loop {
            if *self.held.borrow_and_update() {
                return true;
            }
            if self.held.changed().await.is_err() {
                return false;
            }
        }
    }

    /// Waits until this engine no longer holds the lease.
    pub async fn lost(&mut self) {
        loop {
            if !*self.held.borrow_and_update() {
                return;
            }
            if self.held.changed().await.is_err() {
                return;
            }
        }
    }
}
