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

use crate::config::{ProcessEnv, Vars, parse_enabled, positive};
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use tracing::{info, warn};

use crate::error::Result;

/// Default maximum number of CONSECUTIVE no-progress auto-restarts before an
/// instance is failed terminally. Override with `RUNTARA_MAX_AUTO_RESTARTS`.
pub const DEFAULT_MAX_AUTO_RESTARTS: i32 = 5;

/// Read the configured crash-loop cap (`RUNTARA_MAX_AUTO_RESTARTS`, default
/// [`DEFAULT_MAX_AUTO_RESTARTS`]). Values below 1 fall back to the default.
pub fn max_auto_restarts() -> i32 {
    max_auto_restarts_from(&ProcessEnv)
}

/// [`max_auto_restarts`] against a supplied set of values.
fn max_auto_restarts_from(vars: &dyn Vars) -> i32 {
    positive(vars, "RUNTARA_MAX_AUTO_RESTARTS", DEFAULT_MAX_AUTO_RESTARTS)
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
    auto_recover_enabled_from(&ProcessEnv)
}

/// [`auto_recover_enabled`] against a supplied set of values.
fn auto_recover_enabled_from(vars: &dyn Vars) -> bool {
    parse_enabled(vars.get("RUNTARA_AUTO_RECOVER").as_deref())
}

/// The Environment-wide restart-recovery settings, applied identically to
/// every orphaned instance. There is no per-workflow override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPolicy {
    /// See [`auto_recover_enabled`].
    pub auto_recover: bool,
    /// See [`max_auto_restarts`].
    pub max_auto_restarts: i32,
}

impl RecoveryPolicy {
    /// Read `RUNTARA_AUTO_RECOVER` and `RUNTARA_MAX_AUTO_RESTARTS`.
    pub fn from_env() -> Self {
        Self::from_vars(&ProcessEnv)
    }

    /// [`RecoveryPolicy::from_env`] against a supplied set of values.
    fn from_vars(vars: &dyn Vars) -> Self {
        Self {
            auto_recover: auto_recover_enabled_from(vars),
            max_auto_restarts: max_auto_restarts_from(vars),
        }
    }
}

/// What an Environment with neither variable set runs with.
impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            auto_recover: true,
            max_auto_restarts: DEFAULT_MAX_AUTO_RESTARTS,
        }
    }
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

/// Recover one observed physical registration only after locking its durable
/// launch. Holding that lock across the Core write prevents queue reconciliation
/// and a new start from replacing the generation midway through recovery.
/// None means a live claim or a replacement owns the observation now.
///
/// Whether a recovered instance is relaunched or failed follows
/// [`recover_or_fail`], i.e. the Environment-wide [`RecoveryPolicy`].
pub async fn recover_registered(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    container: &crate::container_registry::ContainerInfo,
) -> Result<Option<RecoveryOutcome>> {
    recover_registered_with(pool, persistence, container, RecoveryPolicy::from_env()).await
}

/// [`recover_registered`] with the policy supplied rather than read from the
/// process environment.
pub async fn recover_registered_with(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    container: &crate::container_registry::ContainerInfo,
    policy: RecoveryPolicy,
) -> Result<Option<RecoveryOutcome>> {
    let mut guard = pool.begin().await?;
    let launch_state: Option<String> =
        sqlx::query_scalar("SELECT state FROM instance_launches WHERE launch_id = $1 FOR UPDATE")
            .bind(&container.launch_id)
            .fetch_optional(&mut *guard)
            .await?;
    if let Some(state) = launch_state {
        // Evaluate time in a fresh statement after acquiring the row lock.
        let live: bool = sqlx::query_scalar(
            "SELECT lease_owner IS NOT NULL AND \
             COALESCE(lease_expires_at > clock_timestamp(), false) \
             FROM instance_launches WHERE launch_id = $1",
        )
        .bind(&container.launch_id)
        .fetch_one(&mut *guard)
        .await?;
        if live || !matches!(state.as_str(), "running" | "starting") {
            return Ok(None);
        }
    }
    // Legacy registrations may predate the launch queue. Their prior recovery
    // behavior remains; they do not establish cross-version owner liveness.
    let registered: Option<(Option<sqlx::types::Json<crate::observed_exit::ObservedExit>>,)> =
        sqlx::query_as(
            "SELECT observed_exit FROM container_registry \
         WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3 FOR UPDATE",
        )
        .bind(&container.instance_id)
        .bind(&container.launch_id)
        .bind(&container.container_id)
        .fetch_optional(&mut *guard)
        .await?;
    let Some((observed_exit,)) = registered else {
        return Ok(None);
    };
    // An observed physical failure is not a lost Environment process. Complete
    // its retained lifecycle transition before considering normal auto-resume.
    let outcome = if let Some(sqlx::types::Json(intent)) = observed_exit {
        if intent.apply(persistence, &container.instance_id).await? {
            if intent.is_drain() {
                RecoveryOutcome::Recovered
            } else {
                RecoveryOutcome::Failed
            }
        } else {
            RecoveryOutcome::Unchanged
        }
    } else {
        match persistence
            .get_instance_meta(&container.instance_id)
            .await?
        {
            Some(instance) if instance.status == runtara_core::domain::InstanceStatus::Running => {
                let outcome =
                    recover_or_fail_with(pool, persistence, &container.instance_id, policy).await?;
                if outcome == RecoveryOutcome::Unchanged {
                    return Ok(Some(outcome));
                }
                outcome
            }
            Some(instance) if instance.status == runtara_core::domain::InstanceStatus::Pending => {
                // The durable start-gate/queue expiry path owns an unopened run.
                return Ok(None);
            }
            _ => RecoveryOutcome::Unchanged,
        }
    };
    sqlx::query(
        "DELETE FROM container_registry \
         WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3",
    )
    .bind(&container.instance_id)
    .bind(&container.launch_id)
    .bind(&container.container_id)
    .execute(&mut *guard)
    .await?;
    guard.commit().await?;
    Ok(Some(outcome))
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
/// fingerprint (checkpoint count), the cap, and the Environment-wide
/// auto-recovery switch.
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
            error: "Killed by Environment restart; automatic recovery is disabled for this \
                    Environment (RUNTARA_AUTO_RECOVER)"
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
/// Recovery is governed by the Environment-wide [`RecoveryPolicy`]; there is
/// no per-workflow override, so every orphaned instance in this Environment
/// gets the same answer.
/// A concurrent lifecycle transition returns `Unchanged`; a failed write
/// returns an error, so callers do not retire tracking as though it succeeded.
pub async fn recover_or_fail(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
) -> Result<RecoveryOutcome> {
    recover_or_fail_with(pool, persistence, instance_id, RecoveryPolicy::from_env()).await
}

/// [`recover_or_fail`] with the policy supplied rather than read from the
/// process environment.
async fn recover_or_fail_with(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
    policy: RecoveryPolicy,
) -> Result<RecoveryOutcome> {
    let RecoveryPolicy {
        auto_recover,
        max_auto_restarts: cap,
    } = policy;
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
    use super::{
        Decision, RecoveryPolicy, auto_recover_enabled_from, decide, max_auto_restarts_from,
    };
    use crate::config::FixedVars;

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
            Decision::Fail { error } => assert_eq!(
                error,
                "Killed by Environment restart; automatic recovery is disabled for this \
                 Environment (RUNTARA_AUTO_RECOVER)"
            ),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    /// The cap rejects anything that would disable crash-loop protection or
    /// make it nonsensical, rather than honouring it.
    ///
    /// A cap of zero fails every restarted instance on its first attempt, which
    /// reads as "recovery is broken" rather than as a configured policy; the
    /// switch for turning recovery off is RUNTARA_AUTO_RECOVER.
    #[test]
    fn the_restart_cap_falls_back_on_anything_non_positive() {
        for value in ["0", "-1", "", "  ", "lots"] {
            let vars = FixedVars::new([("RUNTARA_MAX_AUTO_RESTARTS", value)]);
            assert_eq!(max_auto_restarts_from(&vars), 5, "{value:?}");
        }

        assert_eq!(max_auto_restarts_from(&FixedVars::empty()), 5);
        assert_eq!(
            max_auto_restarts_from(&FixedVars::new([("RUNTARA_MAX_AUTO_RESTARTS", "12")])),
            12
        );
    }

    /// The kill switch answers to the same spellings as every other switch in
    /// the crate; a hand-rolled parser here used to ignore `off`.
    #[test]
    fn auto_recovery_is_on_unless_explicitly_disabled() {
        assert!(auto_recover_enabled_from(&FixedVars::empty()));

        for value in ["false", "0", "no", "off", "disabled", "Off", "  FALSE  "] {
            let vars = FixedVars::new([("RUNTARA_AUTO_RECOVER", value)]);
            assert!(!auto_recover_enabled_from(&vars), "{value:?}");
        }

        for value in ["true", "1", "yes", "on", "typo"] {
            let vars = FixedVars::new([("RUNTARA_AUTO_RECOVER", value)]);
            assert!(auto_recover_enabled_from(&vars), "{value:?}");
        }
    }

    /// The policy reads both variables, and an unset environment yields the
    /// same policy as `Default`.
    #[test]
    fn policy_reads_both_variables_and_defaults_match() {
        assert_eq!(
            RecoveryPolicy::from_vars(&FixedVars::empty()),
            RecoveryPolicy::default()
        );
        assert_eq!(
            RecoveryPolicy::from_vars(&FixedVars::new([
                ("RUNTARA_AUTO_RECOVER", "off"),
                ("RUNTARA_MAX_AUTO_RESTARTS", "9"),
            ])),
            RecoveryPolicy {
                auto_recover: false,
                max_auto_restarts: 9,
            }
        );
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod persistence_tests {
    use super::{RecoveryOutcome, RecoveryPolicy, recover_or_fail_with};
    use crate::instance_repository::InstanceRepository;
    use runtara_store_postgres::PostgresPersistence;

    fn policy(auto_recover: bool) -> RecoveryPolicy {
        RecoveryPolicy {
            auto_recover,
            ..RecoveryPolicy::default()
        }
    }

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
                    recover_or_fail_with(&pool, &persistence, &id, policy(auto_recover))
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
                recover_or_fail_with(&pool, &persistence, &id, policy(auto_recover))
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
                recover_or_fail_with(&pool, &persistence, &id, policy(auto_recover))
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
                recover_or_fail_with(&pool, &persistence, "unavailable", policy(auto_recover))
                    .await
                    .is_err()
            );
        }
    }
}
