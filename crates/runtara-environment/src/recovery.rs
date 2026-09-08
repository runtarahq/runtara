// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Automatic recovery of guest instances killed by an Environment restart.
//!
//! When the Environment restarts — a graceful drain OR an abrupt kill — an
//! in-process guest dies with it. The graceful drain already suspends the
//! instance (`shutdown_requested` + `sleep_until=now`) so the wake scheduler
//! relaunches it. The abrupt path leaves the instance `running` in Core, and on
//! the next startup the orphan scans ([`crate::runtime`] startup recovery and
//! [`crate::heartbeat_monitor`]) would otherwise mark it terminally `failed`
//! with "Process terminated during Environment restart" — never relaunching it.
//!
//! [`recover_or_fail`] routes those would-be failures into the SAME
//! suspend → wake → relaunch path the drain uses. The engine is
//! replay-from-start with checkpoints as a result cache, so a relaunched
//! instance replays from the entry point and completed durable steps are served
//! from cache — i.e. "resume from checkpoint" and "auto re-queue" in one move.
//!
//! A crash-loop cap (`RUNTARA_MAX_AUTO_RESTARTS`) bounds instances that crash
//! before making progress. The cap counts only CONSECUTIVE no-progress
//! restarts: the counter resets whenever the instance's checkpoint count
//! advances between recoveries, so a genuinely long-running workflow survives
//! any number of restarts.

use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use tracing::{info, warn};

use crate::error::Result;

/// Default maximum number of CONSECUTIVE no-progress auto-restarts before an
/// instance is failed terminally. Override with `RUNTARA_MAX_AUTO_RESTARTS`.
pub const DEFAULT_MAX_AUTO_RESTARTS: i32 = 5;

/// Read the configured crash-loop cap (`RUNTARA_MAX_AUTO_RESTARTS`, default
/// [`DEFAULT_MAX_AUTO_RESTARTS`]). Values below 1 fall back to the default.
pub fn max_auto_restarts() -> i32 {
    std::env::var("RUNTARA_MAX_AUTO_RESTARTS")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_MAX_AUTO_RESTARTS)
}

/// Operator-level kill switch for automatic restart recovery. Set
/// `RUNTARA_AUTO_RECOVER` to any of `false`/`0`/`no`/`off`/`disabled` to turn
/// auto-recovery off for the whole Environment — instances killed by a restart
/// then fail terminally with the `environment_restart` reason instead of being
/// relaunched. Defaults to on, matching the always-on graceful-drain recovery.
///
/// Shares [`parse_enabled`](crate::config::parse_enabled) with
/// the `*_CLEANUP_ENABLED` opt-outs so every switch in the crate answers to the
/// same spellings; a hand-rolled parser here used to ignore `off`.
pub fn auto_recover_enabled() -> bool {
    crate::config::parse_enabled(std::env::var("RUNTARA_AUTO_RECOVER").ok().as_deref())
}

/// Outcome of a recovery decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Instance was marked for recovery (suspended + due to wake).
    Recovered,
    /// Crash-loop cap exceeded, or auto-recovery disabled: instance failed
    /// terminally with `termination_reason = environment_restart`.
    Failed,
    /// Another lifecycle transition won before the recovery write. No recovery
    /// state was applied and the caller must not claim a restart or failure.
    Unchanged,
}

/// Pure crash-loop decision, separated from any I/O so it can be unit-tested.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    /// Recover, recording this 1-based consecutive-no-progress attempt number.
    Recover { attempt: i32 },
    /// Fail terminally with this operator-facing error string.
    Fail { error: String },
}

/// Decide whether to recover, given the prior counters, the current progress
/// fingerprint (checkpoint count), the cap, and the per-workflow policy.
///
/// The counter resets to 1 when progress advanced since the last recovery (or
/// there was no prior recovery); otherwise it increments. Once it would exceed
/// `cap`, or auto-recovery is disabled, the instance fails.
fn decide(
    prev_attempts: i32,
    prev_marker: Option<&str>,
    progress: i64,
    cap: i32,
    auto_recover: bool,
) -> Decision {
    let made_progress = prev_marker
        .and_then(|m| m.parse::<i64>().ok())
        .map(|p| progress > p)
        .unwrap_or(true);
    let attempt = if made_progress { 1 } else { prev_attempts + 1 };

    if !auto_recover {
        return Decision::Fail {
            error:
                "Killed by Environment restart; automatic recovery is disabled for this workflow"
                    .to_string(),
        };
    }
    if attempt > cap {
        return Decision::Fail {
            error: format!(
                "Killed by Environment restart; exceeded automatic restart limit ({cap})"
            ),
        };
    }
    Decision::Recover { attempt }
}

/// Decide whether to auto-recover an instance that was killed by an Environment
/// restart, or fail it terminally.
///
/// The caller has already determined the instance was orphaned by a restart
/// (its process is gone but Core still shows it `running`). On `Recovered`, the
/// wake scheduler relaunches the instance on its next poll. On `Failed`, the
/// instance is left in a terminal state with a clear operator-facing reason.
///
/// `auto_recover` is the per-workflow policy (default `true`; Phase 3 wires the
/// real value through from the workflow definition).
/// A concurrent lifecycle transition returns `Unchanged`; a failed write
/// returns an error, so callers do not retire tracking as though it succeeded.
pub async fn recover_or_fail(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
    auto_recover: bool,
) -> Result<RecoveryOutcome> {
    // Progress fingerprint: total checkpoints written for this instance.
    // Monotonic, so a higher count than the last recovery means the instance
    // made forward progress across the restart.
    let progress = persistence
        .count_checkpoints(instance_id, None, None, None)
        .await
        .unwrap_or(0);
    let marker = progress.to_string();

    // Prior crash-loop counters (best-effort; treat read failure as a fresh
    // instance so we err toward recovering rather than failing).
    let (prev_attempts, prev_marker) = match persistence.get_instance_meta(instance_id).await {
        Ok(Some(inst)) => (inst.recovery_attempts, inst.recovery_marker),
        _ => (0, None),
    };

    let cap = max_auto_restarts();

    match decide(
        prev_attempts,
        prev_marker.as_deref(),
        progress,
        cap,
        auto_recover,
    ) {
        Decision::Fail { error: err } => {
            let applied = persistence
                .complete_instance(
                    CompleteInstanceParams::new(
                        instance_id,
                        runtara_core::domain::InstanceStatus::Failed,
                    )
                    .if_running()
                    .with_termination("environment_restart", None)
                    .with_error(&err),
                )
                .await?;
            if !applied {
                return Ok(RecoveryOutcome::Unchanged);
            }
            warn!(
                instance_id = %instance_id,
                cap,
                auto_recover,
                "Instance NOT auto-recovered after Environment restart"
            );
            Ok(RecoveryOutcome::Failed)
        }
        Decision::Recover { attempt } => {
            let applied = crate::instance_repository::InstanceRepository::new(pool.clone())
                .mark_for_recovery(instance_id, attempt, Some(&marker))
                .await?;
            if !applied {
                return Ok(RecoveryOutcome::Unchanged);
            }
            info!(
                instance_id = %instance_id,
                attempt,
                cap,
                progress,
                "Marked instance for automatic recovery after Environment restart (wake scheduler will relaunch)"
            );
            Ok(RecoveryOutcome::Recovered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decision, decide};

    #[test]
    fn first_recovery_starts_at_one() {
        // No prior marker → treated as forward progress → attempt 1.
        assert_eq!(
            decide(0, None, 0, 5, true),
            Decision::Recover { attempt: 1 }
        );
    }

    #[test]
    fn no_progress_increments_toward_cap() {
        // Same checkpoint count as last recovery → no progress → increment.
        assert_eq!(
            decide(1, Some("0"), 0, 5, true),
            Decision::Recover { attempt: 2 }
        );
        assert_eq!(
            decide(4, Some("0"), 0, 5, true),
            Decision::Recover { attempt: 5 }
        );
    }

    #[test]
    fn exceeding_cap_fails() {
        // attempt would be 6 > cap 5 → fail terminally.
        match decide(5, Some("0"), 0, 5, true) {
            Decision::Fail { error } => assert!(error.contains("restart limit (5)")),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn forward_progress_resets_counter() {
        // Checkpoint count advanced (3 > 0) since last recovery → reset to 1
        // even though prev_attempts was at the cap. A long-running workflow
        // that keeps making progress recovers unboundedly.
        assert_eq!(
            decide(5, Some("0"), 3, 5, true),
            Decision::Recover { attempt: 1 }
        );
    }

    #[test]
    fn disabled_auto_recover_always_fails() {
        match decide(0, None, 10, 5, false) {
            Decision::Fail { error } => assert!(error.contains("disabled")),
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod persistence_tests {
    use super::{RecoveryOutcome, recover_or_fail};
    use crate::instance_repository::InstanceRepository;
    use runtara_store_postgres::PostgresPersistence;

    async fn snapshot(pool: &sqlx::PgPool, id: &str) -> serde_json::Value {
        sqlx::query_scalar(
            "SELECT jsonb_build_object(\
             'status', status, 'termination_reason', termination_reason, \
             'sleep_until', sleep_until, 'wake_reason', wake_reason, \
             'finished_at', finished_at, 'output', output, 'error', error, \
             'recovery_attempts', recovery_attempts, 'recovery_marker', recovery_marker) \
             FROM instances WHERE instance_id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read lifecycle snapshot")
    }

    #[tokio::test]
    async fn recovery_write_preserves_states_that_changed_after_scan() {
        let pool = crate::test_support::pool().await;
        let repository = InstanceRepository::new(pool.clone());
        for status in ["cancelled", "completed", "failed", "suspended", "pending"] {
            let id = crate::test_support::unique_id("recovery-stale-scan");
            sqlx::query(
                "INSERT INTO instances (instance_id, tenant_id, status) \
                 VALUES ($1, 'recovery-test', 'running')",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .expect("create running instance");
            // A recovery scan observed Running, but another lifecycle writer
            // settles/parks/requeues it before the recovery UPDATE executes.
            assert_eq!(snapshot(&pool, &id).await["status"], "running");
            sqlx::query(
                "UPDATE instances SET status = $2::instance_status, \
                 termination_reason = 'cancelled', finished_at = NOW(), \
                 sleep_until = NOW() + INTERVAL '1 hour', \
                 recovery_attempts = 3, recovery_marker = '7', \
                 output = $3, error = 'accepted outcome' WHERE instance_id = $1",
            )
            .bind(&id)
            .bind(status)
            .bind(b"accepted result".as_slice())
            .execute(&pool)
            .await
            .expect("settle after scan");
            let before = snapshot(&pool, &id).await;
            assert!(
                !repository
                    .mark_for_recovery(&id, 4, Some("8"))
                    .await
                    .expect("attempt stale recovery")
            );
            assert_eq!(snapshot(&pool, &id).await, before, "state {status}");
            let persistence = PostgresPersistence::new(pool.clone());
            for auto_recover in [true, false] {
                assert_eq!(
                    recover_or_fail(&pool, &persistence, &id, auto_recover)
                        .await
                        .expect("resolve stale recovery"),
                    RecoveryOutcome::Unchanged
                );
                assert_eq!(snapshot(&pool, &id).await, before, "state {status}");
            }
        }
    }

    #[tokio::test]
    async fn recovery_reports_applied_outcome_once() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        for auto_recover in [true, false] {
            let id = crate::test_support::unique_id("recovery-applied");
            sqlx::query(
                "INSERT INTO instances (instance_id, tenant_id, status) \
                 VALUES ($1, 'recovery-test', 'running')",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .expect("create running instance");
            assert_eq!(
                recover_or_fail(&pool, &persistence, &id, auto_recover)
                    .await
                    .expect("apply recovery decision"),
                if auto_recover {
                    RecoveryOutcome::Recovered
                } else {
                    RecoveryOutcome::Failed
                }
            );
            let after = snapshot(&pool, &id).await;
            assert_eq!(
                after["status"],
                if auto_recover { "suspended" } else { "failed" }
            );
            assert_eq!(after["termination_reason"], "environment_restart");
            if auto_recover {
                assert!(!after["sleep_until"].is_null());
                assert_eq!(after["recovery_attempts"], 1);
            }
            assert_eq!(
                recover_or_fail(&pool, &persistence, &id, auto_recover)
                    .await
                    .expect("repeat recovery decision"),
                RecoveryOutcome::Unchanged
            );
            assert_eq!(snapshot(&pool, &id).await, after);
        }
    }

    #[tokio::test]
    async fn recovery_write_failure_is_not_reported_as_terminal_failure() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        pool.close().await;
        for auto_recover in [true, false] {
            assert!(
                recover_or_fail(&pool, &persistence, "unavailable", auto_recover)
                    .await
                    .is_err()
            );
        }
    }
}
