// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Who the engines are: a row each in the runtime database
//! (`engine_members`), written when an engine starts, renewed while it runs
//! and removed when it stops. A row not renewed within [`LEASE`] is an
//! engine that died; any engine removes it.
//!
//! An engine that starts beside a live peer checks it can share with it
//! ([`may_join`]): its artifacts are where the peer can read them, and it
//! runs the peer's release.

use std::time::Duration;

use sqlx::PgPool;
use tracing::{info, warn};

use crate::shutdown::ShutdownSignal;

/// How often an engine renews its row.
pub const RENEW: Duration = Duration::from_secs(10);
/// How long a row stands without being renewed.
pub const LEASE: Duration = Duration::from_secs(30);

/// One engine, as its row says.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Member {
    pub id: String,
    pub release: String,
    pub run_channel_endpoint: Option<String>,
}

/// What this engine says of itself when it joins.
#[derive(Debug, Clone)]
pub struct Me {
    pub id: String,
    pub release: String,
    pub run_channel_endpoint: Option<String>,
}

impl Me {
    /// This process, as an engine.
    pub fn this_engine() -> Self {
        Self {
            id: super::engine_id().to_string(),
            release: super::release(),
            run_channel_endpoint: std::env::var("RUNTARA_RUN_CHANNEL_ENDPOINT")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }
}

/// What lets an engine start beside its peers.
#[derive(Debug, Clone, Copy)]
pub struct Sharing {
    /// Its artifacts are in a store every engine reads
    /// (`RUNTARA_ARTIFACT_STORE_URL`), not on its own disk.
    pub artifacts_shared: bool,
    /// An upgrade is rolling: a peer may run another release for a while
    /// (`RUNTARA_ROLLING_UPGRADE`).
    pub rolling_upgrade: bool,
}

impl Sharing {
    pub fn from_env(artifacts_shared: bool) -> Self {
        Self {
            artifacts_shared,
            rolling_upgrade: std::env::var("RUNTARA_ROLLING_UPGRADE").is_ok_and(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            }),
        }
    }
}

/// Whether `me` may start beside `peers`, the live engines other than
/// itself; `Err` says why not.
pub fn may_join(me: &Me, peers: &[Member], sharing: Sharing) -> Result<(), String> {
    let Some(peer) = peers.first() else {
        return Ok(());
    };
    if !sharing.artifacts_shared {
        return Err(format!(
            "another engine ({}) runs on this database, and this engine's artifacts are on its \
             own disk: an image one engine compiled would be missing on the other. Set \
             RUNTARA_ARTIFACT_STORE_URL to a store every engine reads (docs/multiple-engines-plan.md)",
            peer.id
        ));
    }
    if !sharing.rolling_upgrade
        && let Some(other) = peers.iter().find(|peer| peer.release != me.release)
    {
        return Err(format!(
            "another engine ({}) runs release {}, and this one runs {}: engines on one \
             database run one release. Set RUNTARA_ROLLING_UPGRADE=true while an upgrade rolls",
            other.id, other.release, me.release
        ));
    }
    Ok(())
}

/// The live engines other than `me`.
pub async fn live_peers(pool: &PgPool, me: &str) -> Result<Vec<Member>, sqlx::Error> {
    sqlx::query_as::<_, Member>(
        r#"
        SELECT id, release, run_channel_endpoint
        FROM engine_members
        WHERE id <> $1 AND last_seen > now() - make_interval(secs => $2)
        ORDER BY started_at
        "#,
    )
    .bind(me)
    .bind(LEASE.as_secs_f64())
    .fetch_all(pool)
    .await
}

/// Writes `me`'s row, or renews it.
pub async fn join(pool: &PgPool, me: &Me) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO engine_members (id, release, run_channel_endpoint)
        VALUES ($1, $2, $3)
        ON CONFLICT (id) DO UPDATE SET last_seen = now()
        "#,
    )
    .bind(&me.id)
    .bind(&me.release)
    .bind(&me.run_channel_endpoint)
    .execute(pool)
    .await?;
    Ok(())
}

/// Removes `id`'s row.
pub async fn leave(pool: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM engine_members WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Removes the rows not renewed within [`LEASE`]: engines that died.
pub async fn forget_the_dead(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let removed = sqlx::query(
        "DELETE FROM engine_members WHERE last_seen <= now() - make_interval(secs => $1)",
    )
    .bind(LEASE.as_secs_f64())
    .execute(pool)
    .await?;
    Ok(removed.rows_affected())
}

/// Joins, then renews `me`'s row every [`RENEW`] until shutdown, and leaves.
pub async fn run(pool: PgPool, me: Me, shutdown: ShutdownSignal) {
    loop {
        if let Err(error) = join(&pool, &me).await {
            warn!(error = %error, "could not renew this engine's row in engine_members");
        }
        match forget_the_dead(&pool).await {
            Ok(0) => {}
            Ok(removed) => info!(removed, "forgot engines whose rows were not renewed"),
            Err(error) => warn!(error = %error, "could not forget engines that died"),
        }
        tokio::select! {
            _ = tokio::time::sleep(RENEW) => {}
            _ = shutdown.clone().wait() => {}
        }
        if shutdown.is_shutting_down() {
            if let Err(error) = leave(&pool, &me.id).await {
                warn!(error = %error, "could not remove this engine's row from engine_members");
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(release: &str) -> Me {
        Me {
            id: "engine-me".into(),
            release: release.into(),
            run_channel_endpoint: None,
        }
    }

    fn peer(release: &str) -> Member {
        Member {
            id: "engine-peer".into(),
            release: release.into(),
            run_channel_endpoint: None,
        }
    }

    const SHARED: Sharing = Sharing {
        artifacts_shared: true,
        rolling_upgrade: false,
    };

    #[test]
    fn an_engine_alone_starts_however_it_keeps_artifacts() {
        let alone = Sharing {
            artifacts_shared: false,
            rolling_upgrade: false,
        };
        assert!(may_join(&me("1"), &[], alone).is_ok());
    }

    #[test]
    fn an_engine_with_artifacts_on_its_own_disk_does_not_start_beside_a_peer() {
        let local = Sharing {
            artifacts_shared: false,
            rolling_upgrade: false,
        };
        let error = may_join(&me("1"), &[peer("1")], local).unwrap_err();
        assert!(error.contains("RUNTARA_ARTIFACT_STORE_URL"), "{error}");
    }

    #[test]
    fn engines_run_one_release_unless_an_upgrade_rolls() {
        assert!(may_join(&me("1"), &[peer("1")], SHARED).is_ok());
        let error = may_join(&me("2"), &[peer("1")], SHARED).unwrap_err();
        assert!(error.contains("RUNTARA_ROLLING_UPGRADE"), "{error}");
        let rolling = Sharing {
            rolling_upgrade: true,
            ..SHARED
        };
        assert!(may_join(&me("2"), &[peer("1")], rolling).is_ok());
    }
}
