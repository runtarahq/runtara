// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! One refresh of a connection's tokens at a time, across every engine on
//! one Valkey: `<tenant>:oauth-refresh:<connection>`, held for at most
//! [`HELD_FOR`]. Within one process the token cache's own lock already
//! keeps refreshes one at a time; this keeps them so between processes.
//!
//! The lock holds no token: only a random value, so that only its holder
//! removes it.

use std::time::Duration;

use redis::aio::ConnectionManager;

/// The longest a refresh holds the lock: past it, another may refresh.
pub const HELD_FOR: Duration = Duration::from_secs(30);
/// How often a waiting engine asks again.
const ASKED_EVERY: Duration = Duration::from_millis(100);

/// The lock's key.
pub fn key(tenant_id: &str, connection_id: &str) -> String {
    format!("{tenant_id}:oauth-refresh:{connection_id}")
}

/// A lock this process holds.
#[must_use = "a lock is released by its holder"]
pub struct Held {
    key: String,
    value: String,
}

const RELEASE: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
"#;

/// Takes the lock on `connection_id`'s refresh, waiting while another
/// holds it, for at most [`HELD_FOR`]. `None` when it could not be had (it
/// was still held, or Valkey could not be reached): the refresh goes on
/// without it, as it did in one process, under the rotation guard.
pub async fn take(
    valkey: &ConnectionManager,
    tenant_id: &str,
    connection_id: &str,
) -> Option<Held> {
    let held = Held {
        key: key(tenant_id, connection_id),
        value: uuid::Uuid::new_v4().simple().to_string(),
    };
    let until = tokio::time::Instant::now() + HELD_FOR;
    let mut conn = valkey.clone();
    loop {
        let taken: Result<Option<String>, _> = redis::cmd("SET")
            .arg(&held.key)
            .arg(&held.value)
            .arg("NX")
            .arg("PX")
            .arg(HELD_FOR.as_millis() as u64)
            .query_async(&mut conn)
            .await;
        match taken {
            Ok(Some(_)) => return Some(held),
            Ok(None) if tokio::time::Instant::now() < until => {
                tokio::time::sleep(ASKED_EVERY).await;
            }
            Ok(None) => {
                tracing::warn!(
                    connection_id,
                    "a connection's refresh lock stayed held; refreshing without it"
                );
                return None;
            }
            Err(error) => {
                tracing::warn!(connection_id, %error, "a connection's refresh lock could not be taken; refreshing without it");
                return None;
            }
        }
    }
}

impl Held {
    /// Gives the lock up, if it is still this process's.
    pub async fn release(self, valkey: &ConnectionManager) {
        let mut conn = valkey.clone();
        let released: Result<i64, _> = redis::Script::new(RELEASE)
            .key(&self.key)
            .arg(&self.value)
            .invoke_async(&mut conn)
            .await;
        if let Err(error) = released {
            // It expires on its own.
            tracing::debug!(%error, "a connection's refresh lock was not released");
        }
    }
}
